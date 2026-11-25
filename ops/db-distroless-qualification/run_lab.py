"""Run and clean up the synthetic split-image lab on its scoped Docker daemon."""

import argparse
import hashlib
import json
import secrets
import subprocess
import sys
import traceback
from pathlib import Path

import runtime

from packages import owned_directory

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
FIXTURE = "/tmp/sibling-tls"
DOCKER = ["docker", "--host", "unix:///var/run/docker.sock"]


def docker(*args, data=None, check=True, timeout=45):
    return subprocess.run(
        [*DOCKER, *args], input=data, capture_output=True, check=check, timeout=timeout
    )


def daemon():
    value = docker("info", "--format", "{{.ID}}").stdout.decode().strip()
    runtime.require(value, "Docker daemon has no identity")
    return value


def inspect(kind, identity):
    result = docker(kind, "inspect", identity, check=False)
    if result.returncode:
        diagnostics = {
            f"error: no such object: {identity}",
            f"error response from daemon: no such {kind}: {identity}",
        }
        if kind == "volume":
            diagnostics.add(f"error response from daemon: get {identity}: no such volume")
        runtime.require(
            result.returncode == 1
            and result.stdout.strip() in {b"", b"[]"}
            and result.stderr.decode().strip().lower() in diagnostics,
            "Unexpected Docker inspect failure",
        )
        return None
    (record,) = json.loads(result.stdout)
    return record


class Lab:
    def __init__(self, scope):
        self.scope = scope
        self.created = {}
        self.volume_planned = False

    def engine(self):
        identity = daemon()
        runtime.require(identity == self.scope.daemon, "Docker daemon changed")
        return identity

    def volume(self):
        self.engine()
        record = inspect("volume", self.scope.volume)
        runtime.require(record is not None, "Owned volume is missing")
        runtime.volume_owned(record, self.scope)
        return record

    def create_volume(self):
        self.engine()
        name = self.scope.volume
        runtime.require(inspect("volume", name) is None, "Scoped volume already exists")
        self.volume_planned = True
        docker(
            "volume",
            "create",
            "--driver",
            "local",
            "--label",
            f"{runtime.LABEL}={self.scope.nonce}",
            name,
        )
        self.volume()

    def record(self, role):
        self.engine()
        known = self.created[role]
        current = inspect("container", known or self.scope.name(role))
        if current is None:
            return None
        identity = known or current["Id"]
        server_id = self.created["server"] if role == "client" else None
        runtime.container_owned(current, self.scope, role, identity, server_id)
        self.created[role] = (
            identity  # Lost create responses recover only the exact owned resource.
        )
        return current

    def owned(self, role):
        record = self.record(role)
        runtime.require(record is not None, "Owned container is missing")
        return record

    def create(self, role):
        self.engine()
        runtime.require(
            role not in self.created and inspect("container", self.scope.name(role)) is None,
            "Scoped container already exists",
        )
        server = self.owned("server") if role == "client" else None
        args = runtime.create_args(
            self.scope,
            role,
            daemon_id=self.engine(),
            volume=self.volume(),
            server_id=server["Id"] if server else None,
            server=server,
        )
        self.created[role] = None
        identity = docker(*args).stdout.decode().strip()
        runtime.require(runtime.instance_id(identity), "Unexpected container identity")
        self.created[role] = identity
        self.owned(role)

    def start(self, role):
        self.volume()
        docker("start", self.owned(role)["Id"])
        self.owned(role)

    def stop(self, role):
        self.volume()
        record = self.owned(role)
        if record["State"]["Running"]:
            docker("stop", "--time", "30", record["Id"])
        return self.owned(role)

    def remove(self, role):
        if role not in self.created:
            return
        record = self.record(role)
        if record is not None:
            if record["State"]["Running"]:
                record = self.stop(role)
            server = self.owned("server") if role == "client" else None
            args = runtime.container_remove_args(
                self.scope,
                role,
                record["Id"],
                record,
                daemon_id=self.engine(),
                volume=self.volume(),
                server_id=server["Id"] if server else None,
                server=server,
            )
            docker(*args)
            runtime.require(self.record(role) is None, "Scoped container still exists")
        del self.created[role]

    def cleanup(self):
        for role in ("client", "server", "bootstrap", "prepare"):
            self.remove(role)
        if self.volume_planned:
            self.engine()
            volume = inspect("volume", self.scope.volume)
            if volume is not None:
                claims = docker(
                    "container", "ls", "-aq", "--filter", "volume=" + self.scope.volume
                ).stdout
                runtime.require(not claims.strip(), "Scoped volume is still claimed")
                docker(*runtime.volume_remove_args(self.scope, self.engine(), self.volume()))
                runtime.require(
                    inspect("volume", self.scope.volume) is None, "Scoped volume still exists"
                )

    def prepare(self):
        self.create_volume()
        self.create("prepare")
        self.start("prepare")
        identity = self.owned("prepare")["Id"]
        runtime.require(
            docker("wait", identity).stdout.strip() == b"0", "Volume preparation failed"
        )
        self.remove("prepare")

    def execute(self, role, *args, data=None, timeout=45):
        self.volume()
        record = self.owned(role)
        runtime.require(
            record["State"]["Running"] is True, "Assigned tool container is not running"
        )
        if role == "client":
            runtime.require(
                self.owned("server")["State"]["Running"] is True, "Owned client server stopped"
            )
        return docker("exec", "-i", record["Id"], *args, data=data, timeout=timeout).stdout

    def bash(self, role, *args, data=None, timeout=45):
        return self.execute(
            role, runtime.BASH, "--noprofile", "--norc", "-p", *args, data=data, timeout=timeout
        )

    def fixture_inputs(self, role, private, names):
        self.bash(
            role,
            "-ec",
            f"umask 077; mkdir -m 700 {FIXTURE}; "
            f'test "$(stat -c %u:%g:%a {FIXTURE})" = 65532:65532:700',
        )
        for name in names:
            local = private / name
            if name == "diagnostics.sh":
                local = REPO / "ops/db-sibling-postgres-tls/diagnostics.sh"
            runtime.require(
                local.is_file() and not local.is_symlink() and 0 < local.stat().st_size <= 16384,
                "Unbounded fixture input",
            )
            payload = local.read_bytes()
            remote = f"{FIXTURE}/{name}"
            received = (
                self.bash(
                    role,
                    "-ec",
                    'umask 077; set -o noclobber; cat > "$1"; sha256sum "$1"',
                    "_",
                    remote,
                    data=payload,
                )
                .decode()
                .split()
            )
            runtime.require(
                received == [hashlib.sha256(payload).hexdigest(), remote],
                "Bounded fixture transfer differs",
            )

    def expect_start_failure(self, expected):
        self.create("server")
        identity = self.owned("server")["Id"]
        # A failed process may exit before docker start returns. Re-read the exact object.
        docker("start", identity, check=False)
        docker("wait", identity)
        state = self.owned("server")["State"]
        runtime.require(
            state["Running"] is False and state["ExitCode"] != 0 and not state["OOMKilled"],
            "Startup negative control did not exit with the expected failure",
        )
        logs = docker("logs", identity)
        runtime.require(
            expected in logs.stdout + logs.stderr, "Startup control failed for another reason"
        )
        self.remove("server")

    def direct_postgres(self):
        server = self.owned("server")
        pid = server["State"]["Pid"]
        runtime.require(type(pid) is int and pid > 0, "Unusable scoped server PID")
        proc = Path("/proc") / str(pid)
        status = {
            key: value.split()
            for line in (proc / "status").read_text().splitlines()
            if (parts := line.partition(":"))[1]
            for key, _, value in [parts]
        }
        runtime.direct_server(server, self.scope, server["Id"], pid, (proc / "comm").read_text())
        runtime.require(
            status["NSpid"][-1] == "1"
            and status["Uid"] == status["Gid"] == ["65532"] * 4
            and "0" not in status.get("Groups", []),
            "Unexpected live PostgreSQL PID 1 identity",
        )
        runtime.require(
            self.owned("server")["State"]["Pid"] == pid, "Scoped PID changed during inspection"
        )

    def client(self, private, phase):
        self.create("client")
        self.start("client")
        self.fixture_inputs(
            "client", private, ("password", "ca.crt", "wrong-ca.crt", "diagnostics.sh")
        )
        result = self.bash(
            "client", "-s", "--", phase, data=(HERE / "tls-client.sh").read_bytes(), timeout=90
        )
        runtime.tls_verdict(result.decode())
        self.remove("client")

    def exercise(self, private):
        self.prepare()
        self.expect_start_failure(b"postgresql.conf")
        self.create("bootstrap")
        self.start("bootstrap")
        self.fixture_inputs("bootstrap", private, ("password", "server.crt", "server.key"))
        (private / "server.key").unlink()
        result = self.bash("bootstrap", "-s", data=(HERE / "bootstrap.sh").read_bytes())
        runtime.require(result == b"synthetic_bootstrap=pass\n", "Synthetic initialization failed")
        self.remove("bootstrap")
        self.create("server")
        self.start("server")
        self.direct_postgres()
        self.client(private, "first")
        stopped = self.stop("server")
        logs = docker("logs", stopped["Id"])
        runtime.require(
            stopped["State"]["ExitCode"] == 0
            and b"database system is shut down" in logs.stdout + logs.stderr,
            "Clean isolated shutdown was not observed",
        )
        self.start("server")
        self.direct_postgres()
        self.client(private, "restart")
        self.stop("server")


def run(server_id, tools_id, private, output, cleanup_receipt, pinned):
    owned_directory(private.parent)
    owned_directory(private)
    owned_directory(output.parent)
    runtime.require(daemon() == pinned, "Unexpected lab daemon")
    control = Lab(
        runtime.Scope(secrets.token_hex(12), server_id, tools_id, pinned, data_owner="0:0")
    )
    lab = Lab(runtime.Scope(secrets.token_hex(12), server_id, tools_id, pinned))
    try:
        control.prepare()
        control.expect_start_failure(b"Permission denied")
        lab.exercise(private)
    finally:
        try:
            lab.cleanup()
        finally:
            control.cleanup()
        with cleanup_receipt.open("x") as receipt:
            receipt.write("success\n")
    output.write_text(
        "\n".join(
            (
                "wrong_owner=pass",
                "uninitialized=pass",
                "pid1=pass",
                *runtime.TLS_RESULTS,
                "transaction_commit_and_rollback=pass",
                "clean_stop=pass",
                "transaction_visible_after_restart=pass",
                "cleanup=pass",
            )
        )
        + "\n"
    )


def failure_sites(error):
    allowed = {str(HERE / name): name for name in ("packages.py", "run_lab.py", "runtime.py")}
    for _ in range(3):
        if error is None:
            break
        sites = [
            (allowed[frame.f_code.co_filename], line)
            for frame, line in traceback.walk_tb(error.__traceback__)
            if frame.f_code.co_filename in allowed and frame.f_code.co_name != "require"
        ]
        yield from reversed(sites[-2:])
        error = error.__cause__ or error.__context__


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("server_id")
    parser.add_argument("tools_id")
    parser.add_argument("private", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("cleanup_receipt", type=Path)
    parser.add_argument("daemon_id")
    args = parser.parse_args()
    try:
        run(
            args.server_id,
            args.tools_id,
            args.private,
            args.output,
            args.cleanup_receipt,
            args.daemon_id,
        )
    except Exception as error:
        for name, line in failure_sites(error):
            print(f"runtime_failure={name}:{line}", file=sys.stderr, flush=True)
        raise
