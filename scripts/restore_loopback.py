"""Bounded loopback relays and identity checks for disposable Docker databases."""

import ipaddress
import json
import re
import select
import socket
import socketserver
import threading

from with_postgres import run


class IsolationFailure(ValueError):
    """A fixed, non-sensitive Docker isolation invariant failed."""


def suitable_private_ipv4(value):
    """Accept only private IPv4 without special-use addressing flags."""
    return (
        value.version == 4
        and value.is_private
        and not any(
            (
                value.is_loopback,
                value.is_link_local,
                value.is_reserved,
                value.is_unspecified,
                value.is_multicast,
            )
        )
    )


class RelayHandler(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            with socket.create_connection(self.server.target, timeout=5) as remote:
                remote.settimeout(10)
                self.request.settimeout(10)
                active = [self.request, remote]
                while active and not self.server.stopping.is_set():
                    ready, _, _ = select.select(active, [], [], 0.5)
                    for source in ready:
                        data = source.recv(65536)
                        destination = remote if source is self.request else self.request
                        if not data:
                            active.remove(source)
                            try:
                                destination.shutdown(socket.SHUT_WR)
                            except OSError:
                                pass
                        else:
                            destination.sendall(data)
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
        raise IsolationFailure("network_inspect_shape")
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
        raise IsolationFailure("network_identity_or_membership")
    configs = network["IPAM"]["Config"]
    if not isinstance(configs, list) or len(configs) != 1:
        raise IsolationFailure("network_subnet_shape")
    subnet = ipaddress.ip_network(configs[0]["Subnet"], strict=True)
    if not all(
        suitable_private_ipv4(value)
        for value in (subnet, subnet.network_address, subnet.broadcast_address)
    ):
        raise IsolationFailure("network_subnet_policy")
    addresses = []
    for container, image in zip(containers, images, strict=True):
        records = json.loads(run("docker", "inspect", container).stdout)
        if not isinstance(records, list) or len(records) != 1:
            raise IsolationFailure("container_inspect_shape")
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
        ):
            raise IsolationFailure("container_identity_or_state")
        if info["HostConfig"].get("PortBindings") not in ({}, None):
            raise IsolationFailure("container_host_bindings")
        if not isinstance(settings["Ports"], dict):
            raise IsolationFailure("container_ports_shape")
        if any(settings["Ports"].values()):
            raise IsolationFailure("container_published_ports")
        if set(connections) != {network_name} or connections[network_name]["NetworkID"] != net_id:
            raise IsolationFailure("container_network_identity")
        address = ipaddress.ip_address(connections[network_name]["IPAddress"])
        member_address = ipaddress.ip_interface(members[container]["IPv4Address"])
        if (
            not suitable_private_ipv4(address)
            or address not in subnet
            or address in (subnet.network_address, subnet.broadcast_address)
            or member_address.ip != address
            or member_address.network != subnet
        ):
            raise IsolationFailure("container_ipv4_identity")
        addresses.append(str(address))
    if len(set(addresses)) != 2:
        raise IsolationFailure("duplicate_container_ipv4")
    return addresses
