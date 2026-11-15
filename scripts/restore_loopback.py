"""Bounded loopback relays and identity checks for disposable Docker databases."""

import ipaddress
import json
import re
import select
import socket
import socketserver
import threading

from with_postgres import run


class RelayHandler(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            with socket.create_connection(self.server.target, timeout=5) as remote:
                remote.settimeout(10)
                self.request.settimeout(10)
                while not self.server.stopping.is_set():
                    ready, _, _ = select.select([self.request, remote], [], [], 0.5)
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        (remote if source is self.request else self.request).sendall(data)
        except OSError:
            # Closing a failed relay forces the fixture's own database operation to fail.
            return


class RelayServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = False
    allow_reuse_address = False
    request_queue_size = 16

    def __init__(self, target, target_port=5432):
        self.target = (target, target_port)
        self.stopping = threading.Event()
        self.slots = threading.BoundedSemaphore(32)
        super().__init__(("127.0.0.1", 0), RelayHandler)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()

    def close(self):
        self.stopping.set()
        self.shutdown()
        self.server_close()
        self.thread.join(timeout=10)
        if self.thread.is_alive():
            raise RuntimeError("Loopback relay did not stop")


def verified_addresses(network_name, marker, containers, images):
    """Return endpoints only for the exact labelled internal network and images."""
    networks = json.loads(run("docker", "network", "inspect", network_name).stdout)
    if not isinstance(networks, list) or len(networks) != 1:
        raise ValueError("Unexpected network inspection")
    network = networks[0]
    net_id = network["Id"]
    members = network["Containers"]
    labels = network.get("Labels")
    if (
        network["Name"] != network_name
        or not re.fullmatch(r"[0-9a-f]{64}", net_id)
        or network["Driver"] != "bridge"
        or network["Internal"] is not True
        or not isinstance(labels, dict)
        or labels.get("sark.restore.owner") != marker
        or not isinstance(members, dict)
        or set(members) != set(containers)
        or len(containers) != 2
    ):
        raise ValueError("Unexpected owned internal network")
    configs = network["IPAM"]["Config"]
    if not isinstance(configs, list) or len(configs) != 1:
        raise ValueError("Unexpected internal subnet")
    subnet = ipaddress.ip_network(configs[0]["Subnet"], strict=True)
    private_ranges = ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")
    if subnet.version != 4 or not any(
        subnet.subnet_of(ipaddress.ip_network(block)) for block in private_ranges
    ):
        raise ValueError("Unexpected internal IPv4 subnet")
    addresses = []
    for container, image in zip(containers, images, strict=True):
        records = json.loads(run("docker", "inspect", container).stdout)
        if not isinstance(records, list) or len(records) != 1:
            raise ValueError("Unexpected container inspection")
        info = records[0]
        settings = info["NetworkSettings"]
        connections = settings["Networks"]
        labels = info["Config"].get("Labels")
        if (
            info["Id"] != container
            or info["Image"] != image
            or info["State"].get("Running") is not True
            or not isinstance(labels, dict)
            or labels.get("sark.restore.owner") != marker
            or info["HostConfig"].get("PortBindings") not in ({}, None)
            or not isinstance(settings["Ports"], dict)
            or any(settings["Ports"].values())
            or set(connections) != {network_name}
            or connections[network_name]["NetworkID"] != net_id
        ):
            raise ValueError("Unexpected container identity or network")
        address = ipaddress.ip_address(connections[network_name]["IPAddress"])
        member_address = ipaddress.ip_interface(members[container]["IPv4Address"])
        if (
            address.version != 4
            or address not in subnet
            or address in (subnet.network_address, subnet.broadcast_address)
            or member_address.ip != address
            or member_address.network != subnet
        ):
            raise ValueError("Unexpected container IPv4 address")
        addresses.append(str(address))
    if len(set(addresses)) != 2:
        raise ValueError("Duplicate container IPv4 address")
    return addresses
