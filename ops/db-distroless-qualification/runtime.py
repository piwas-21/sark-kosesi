"""Pure Docker argument and ownership checks for a split PostgreSQL lab."""

import re
from dataclasses import dataclass

LABEL = "org.piwas.qualification.distroless-postgres"
DATA = "/var/lib/postgresql/18/docker"
VOLUME_TARGET = "/var/lib/postgresql"
POSTGRES = "/opt/pg-smoke/bin/postgres"
USER = "65532:65532"
TMPFS = "rw,nosuid,nodev,mode=1777,size=32m"
BASH = "/usr/bin/bash"
CHOWN = "CAP_CHOWN"
TLS_RESULTS = (
    "verify_full=pass",
    "scram_and_hba=pass",
    "wrong_ca=pass",
    "wrong_host=pass",
    "no_tls=pass",
    "wrong_password=pass",
    "healthy_after_negatives=pass",
)


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def instance_id(value):
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value)


@dataclass(frozen=True)
class Scope:
    nonce: str
    server: str
    tools: str
    daemon: str
    data_owner: str = USER

    def __post_init__(self):
        require(
            re.fullmatch(r"[a-f0-9]{24}", self.nonce)
            and re.fullmatch(r"sha256:[a-f0-9]{64}", self.server)
            and re.fullmatch(r"sha256:[a-f0-9]{64}", self.tools)
            and self.server != self.tools
            and re.fullmatch(r"\S{1,128}", self.daemon)
            and self.data_owner in {USER, "0:0"},
            "Invalid split-image scope",
        )

    @property
    def volume(self):
        return f"sark-pg-distroless-{self.nonce}-data"

    def name(self, role):
        require(role in {"prepare", "server", "bootstrap", "client"}, "Unknown qualification role")
        return f"sark-pg-distroless-{self.nonce}-{role}"


def role_parameters(scope, role, server_id=None):
    scope.name(role)
    if role == "prepare":
        setup = (
            f'test -z "$(ls -A {VOLUME_TARGET})"; '
            f"mkdir -m 755 {VOLUME_TARGET}/18; mkdir -m 700 {DATA}"
        )
        if scope.data_owner == USER:
            setup += f"; chown {USER} {DATA}"
        return scope.tools, "none", BASH, ["-pec", setup]
    if role == "server":
        return (
            scope.server,
            "none",
            POSTGRES,
            [
                "-D",
                DATA,
                "-c",
                "listen_addresses=127.0.0.1",
                "-c",
                "unix_socket_directories=/tmp",
                "-c",
                "shared_buffers=16MB",
                "-c",
                "ssl=on",
            ],
        )
    if role == "client":
        require(instance_id(server_id), "A verified server container ID is required")
        return scope.tools, f"container:{server_id}", "/usr/bin/sleep", ["600"]
    return scope.tools, "none", "/usr/bin/sleep", ["600"]


def scoped_inputs(scope, daemon_id, volume):
    require(daemon_id == scope.daemon, "Docker daemon identity differs from the approved lab")
    volume_owned(volume, scope)


def create_args(scope, role, *, daemon_id, volume, server_id=None, server=None):
    scoped_inputs(scope, daemon_id, volume)
    if role == "client":
        require(server is not None, "A fresh inspected server is required")
        container_owned(server, scope, "server", server_id)
        require(
            server["State"]["Running"] is True and server["State"]["OOMKilled"] is False,
            "The exact owned server must be running",
        )
    image, network, entrypoint, command = role_parameters(scope, role, server_id)
    args = [
        "create",
        "--name",
        scope.name(role),
        "--label",
        f"{LABEL}={scope.nonce}",
        "--pull=never",
        "--network",
        network,
        "--ipc",
        "private",
        "--user",
        "0:0" if role == "prepare" else USER,
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--memory",
        "512m",
        "--memory-swap",
        "512m",
        "--cpus",
        "1",
        "--pids-limit",
        "128",
        "--tmpfs",
        f"/tmp:{TMPFS}",
    ]
    if role == "prepare" and scope.data_owner == USER:
        args += ["--cap-add", CHOWN]
    if role != "client":
        args += ["--mount", f"type=volume,src={scope.volume},dst={VOLUME_TARGET},volume-nocopy"]
    if role == "server":
        args += ["--stop-signal", "SIGINT", "--stop-timeout", "30"]
    return args + ["--entrypoint", entrypoint, image, *command]


def volume_owned(volume, scope):
    require(
        volume["Name"] == scope.volume
        and volume["Driver"] == "local"
        and volume["Scope"] == "local"
        and (volume["Labels"] or {}).get(LABEL) == scope.nonce
        and not volume.get("Options"),
        "Foreign or unverified qualification volume",
    )


def container_owned(record, scope, role, identity, server_id=None):
    image, network, entrypoint, command = role_parameters(scope, role, server_id)
    config, host = record["Config"], record["HostConfig"]
    capabilities = {CHOWN} if role == "prepare" and scope.data_owner == USER else set()
    reason = "Foreign or unverified qualification container"
    require(
        instance_id(identity)
        and record["Id"] == identity
        and record["Image"] == image
        and record["Name"] == "/" + scope.name(role)
        and (config["Labels"] or {}).get(LABEL) == scope.nonce,
        reason,
    )
    require(config["User"] == ("0:0" if role == "prepare" else USER), reason)
    require(config["Entrypoint"] == [entrypoint] and config["Cmd"] == command, reason)
    require(role != "server" or config["StopSignal"] == "SIGINT", reason)
    require(host["NetworkMode"] == network, reason)
    require(
        host.get("PidMode", "") == ""
        and host.get("IpcMode") == "private"
        and host.get("UTSMode", "") == ""
        and host.get("UsernsMode", "") == "",
        reason,
    )
    require(
        not host.get("PortBindings")
        and not host.get("PublishAllPorts")
        and not any((record["NetworkSettings"].get("Ports") or {}).values()),
        reason,
    )
    require(
        host["ReadonlyRootfs"] is True
        and host["Privileged"] is False
        and not host.get("Devices")
        and not host.get("DeviceRequests")
        and not host.get("DeviceCgroupRules")
        and not host.get("GroupAdd")
        and not host.get("Binds")
        and not host.get("VolumesFrom"),
        reason,
    )
    require(host["Memory"] == host["MemorySwap"] == 512 * 1024**2, reason)
    require(host["NanoCpus"] == 10**9 and host["PidsLimit"] == 128, reason)
    require(host["Tmpfs"] == {"/tmp": TMPFS}, reason)
    require(set(host["CapDrop"] or ()) == {"ALL"}, reason)
    require(set(host.get("CapAdd") or ()) == capabilities, reason)
    require(host["SecurityOpt"] in (["no-new-privileges"], ["no-new-privileges:true"]), reason)
    mounts = [
        (item["Type"], item.get("Name"), item["Destination"], item["RW"])
        for item in record["Mounts"]
        if item["Type"] != "tmpfs"
    ]
    want = [] if role == "client" else [("volume", scope.volume, VOLUME_TARGET, True)]
    requested = host.get("Mounts") or []
    require(
        mounts == want
        and len(requested) == len(want)
        and all(
            item["Type"] != "tmpfs" or item["Destination"] == "/tmp" for item in record["Mounts"]
        ),
        "Qualification container mounts differ",
    )
    for mount in requested:
        # Docker serializes a blank DriverConfig when the CLI sets volume-nocopy.
        options = mount.get("VolumeOptions")
        require(
            mount["Type"] == "volume"
            and mount["Source"] == scope.volume
            and mount["Target"] == VOLUME_TARGET
            and mount.get("ReadOnly", False) is False
            and isinstance(options, dict)
            and options.get("NoCopy") is True
            and options.keys() <= {"NoCopy", "DriverConfig"}
            and options.get("DriverConfig", {}) == {},
            "Qualification container volume options differ",
        )


def container_remove_args(
    scope, role, identity, record, *, daemon_id, volume, server_id=None, server=None
):
    scoped_inputs(scope, daemon_id, volume)
    if role == "client":
        require(server is not None, "A fresh inspected server is required")
        container_owned(server, scope, "server", server_id)
    container_owned(record, scope, role, identity, server_id)
    require(record["State"]["Running"] is False, "Stop the exact owned container before removal")
    return ["container", "rm", identity]


def volume_remove_args(scope, daemon_id, volume):
    scoped_inputs(scope, daemon_id, volume)
    return ["volume", "rm", scope.volume]


def direct_server(record, scope, identity, observed_pid, comm):
    container_owned(record, scope, "server", identity)
    state = record["State"]
    require(
        state["Running"] is True
        and state["OOMKilled"] is False
        and type(observed_pid) is int
        and observed_pid > 0
        and type(state["Pid"]) is int
        and state["Pid"] == observed_pid
        and comm.strip() == "postgres",
        "Direct PostgreSQL PID 1 failed",
    )


def tls_verdict(output):
    require(tuple(output.splitlines()) == TLS_RESULTS, "Incomplete or failed TLS/SCRAM controls")
