#!/usr/bin/env python3
"""Build and exercise the real decoder service on a disposable Linux CI runner."""

import json
import os
import re
import secrets
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path

ROOT = Path(__file__).parent.parent
MAX_RESPONSE = 20 + 3 * (20 + 250 * 1024)


def run(args, *, env=None, capture=False, check=True, cwd=ROOT, input_text=None):
    return subprocess.run(
        args, cwd=cwd, env=env, input=input_text, text=True, capture_output=capture, check=check
    )


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def fixture():
    rows = (b"\0" + b"\xff\x00\x00\xff" * 2) * 3
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 3, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


def request(operation, payload, kind=2, width=2, height=3):
    return (
        b"IC01"
        + bytes([operation, kind, 0, 0])
        + struct.pack(">III", width, height, len(payload))
        + payload
    )


def exchange(path, message):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(25)
        client.connect(path)
        client.sendall(message)
        response = bytearray()
        while True:
            try:
                part = client.recv(min(65536, MAX_RESPONSE + 1 - len(response)))
            except ConnectionResetError:
                break
            if not part:
                break
            response.extend(part)
            if len(response) > MAX_RESPONSE:
                raise AssertionError("oversized decoder response")
        return bytes(response)


def prepared_bytes(response):
    if len(response) < 40 or response[:8] != b"ICR1\x02\0\0\0":
        raise AssertionError(
            f"prepare did not return a prepared image (length={len(response)}, "
            f"header={response[:8].hex()})"
        )
    width, height, count = struct.unpack_from(">III", response, 8)
    edge, rw, rh = struct.unpack_from(">III", response, 20)
    kind = response[32:36]
    length = struct.unpack_from(">I", response, 36)[0]
    payload = response[40:]
    if (
        (width, height, count, edge, rw, rh) != (2, 3, 1, 320, 2, 3)
        or kind != b"\x02\0\0\0"
        or length != len(payload)
        or not payload.startswith(b"\x89PNG\r\n\x1a\n")
    ):
        raise AssertionError("invalid prepared rendition")
    return payload


def client(mode, path):
    if mode == "unauthorized":
        if os.getuid() != 1002 or os.getgid() != 1000:
            raise AssertionError("wrong unauthorized client identity")
        # Connection must succeed at the filesystem layer, proving peer UID rejects it.
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
            peer.settimeout(10)
            peer.connect(path)
            try:
                peer.sendall(request(1, fixture()))
                result = peer.recv(64)
            except (BrokenPipeError, ConnectionResetError):
                result = b""
            if result:
                raise AssertionError("unauthorized peer received a response")
        return
    if os.getuid() != 1000 or os.getgid() != 1000:
        raise AssertionError("wrong API client identity")
    image = fixture()
    derivative = prepared_bytes(exchange(path, request(1, image)))
    if exchange(path, request(2, derivative)) != b"ICR1\0\0\0\0":
        raise AssertionError("valid rendition was not verified")
    rejected = exchange(path, b"BAD!" + request(1, image)[4:])
    if rejected not in (b"", b"ICR1\x01\x06\0\0"):
        raise AssertionError("malformed request produced a valid response")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
        peer.settimeout(10)
        peer.connect(path)
        peer.sendall(request(1, image))
    deadline = time.monotonic() + 20
    while True:
        try:
            result = exchange(path, request(1, image))
            if result in (b"", b"ICR1\x01\x06\0\0") and time.monotonic() < deadline:
                time.sleep(0.1)
                continue
            prepared_bytes(result)
            break
        except (ConnectionResetError, BrokenPipeError):
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.1)
    print("API UID: prepare, validate, malformed input and disconnect recovery passed")


def assert_runtime(container):
    info = json.loads(run(["docker", "inspect", container], capture=True).stdout)[0]
    config, host, state = info["Config"], info["HostConfig"], info["State"]
    if not state["Running"] or config["User"] != "65532:1000":
        raise AssertionError("container is not running as the configured decoder identity")
    expected = {
        "NetworkMode": "none",
        "ReadonlyRootfs": True,
        "Memory": 1073741824,
        "MemorySwap": 1073741824,
        "NanoCpus": 1000000000,
        "PidsLimit": 32,
    }
    for key, value in expected.items():
        if host.get(key) != value:
            raise AssertionError(f"unexpected container {key}: {host.get(key)!r}")
    if "ALL" not in host.get("CapDrop", []) or not any(
        item in ("no-new-privileges", "no-new-privileges:true")
        for item in host.get("SecurityOpt", [])
    ):
        raise AssertionError("missing capability or privilege restrictions")
    pid = state["Pid"]
    status = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        key, _, value = line.partition(":")
        status[key] = value.strip()
    if (
        status["Uid"].split()[:2] != ["65532", "65532"]
        or status["Gid"].split()[:2] != ["1000", "1000"]
        or int(status["CapEff"], 16) != 0
        or status["NoNewPrivs"] != "1"
        or status["Seccomp"] != "2"
    ):
        raise AssertionError("live daemon identity or privilege state differs")
    mounts = run(["sudo", "-n", "cat", f"/proc/{pid}/mountinfo"], capture=True).stdout.splitlines()
    root = [line.split() for line in mounts if line.split()[4] == "/"]
    if len(root) != 1 or "ro" not in root[0][5].split(","):
        raise AssertionError("live root mount is not read-only")
    links = json.loads(
        run(
            ["sudo", "-n", "nsenter", "-t", str(pid), "-n", "ip", "-j", "link"], capture=True
        ).stdout
    )
    if [link["ifname"] for link in links] != ["lo"]:
        raise AssertionError("decoder network namespace has non-loopback interfaces")
    cgroups = Path(f"/proc/{pid}/cgroup").read_text().splitlines()
    unified = [line[3:] for line in cgroups if line.startswith("0::/")]
    if len(unified) != 1:
        raise AssertionError("expected a unified cgroup-v2 hierarchy")
    group = Path("/sys/fs/cgroup") / unified[0].lstrip("/")
    expected_files = {"memory.max": "1073741824", "memory.swap.max": "0", "pids.max": "32"}
    for name, value in expected_files.items():
        actual = (group / name).read_text().strip()
        if actual != value:
            raise AssertionError(f"unexpected cgroup {name}: {actual}")
    quota, period = (group / "cpu.max").read_text().split()
    if quota == "max" or int(quota) != int(period):
        raise AssertionError("cgroup CPU quota differs from one CPU")
    print("Live daemon UID, capabilities, root mount, network and cgroups verified")


def main():
    revision = os.environ["SMOKE_REVISION"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("invalid source revision")
    actual = run(["git", "rev-parse", "HEAD"], capture=True).stdout.strip()
    if actual != revision:
        raise ValueError("checkout is not the requested revision")
    runner_temp = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    metadata = runner_temp.stat()
    if not runner_temp.is_dir() or metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022:
        raise ValueError("decoder smoke requires a protected runner directory")
    temp = Path(tempfile.mkdtemp(prefix="decoder-smoke-", dir=runner_temp))
    temp.chmod(0o711)
    socket_dir = temp / "socket"
    client_script = Path(__file__).read_text()
    project = "decoder-smoke-" + os.urandom(6).hex()
    tag = f"sark-kosesi-image-decoder:{revision}"
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("COMPOSE_", "STAGING_"))
    }
    env.update(
        {
            "STAGING_REVISION": revision,
            "STAGING_IMAGE_DECODER_DIR": str(socket_dir),
            "STAGING_DATABASE_PASSWORD": secrets.token_urlsafe(32),
            "STAGING_HOST": "example.invalid",
            "STAGING_PREVIEW_TOKEN": secrets.token_urlsafe(32),
        }
    )
    compose = [
        "docker",
        "compose",
        "--env-file",
        "/dev/null",
        "-p",
        project,
        "-f",
        "deploy/staging/compose.yml",
        "-f",
        "deploy/staging/image-decoder.compose.yml",
    ]
    container = None
    image_built = False
    try:
        run(
            [
                "sudo",
                "-n",
                "install",
                "-d",
                "-m",
                "0750",
                "-o",
                "65532",
                "-g",
                "1000",
                str(socket_dir),
            ]
        )
        run(
            [
                "docker",
                "build",
                "--file",
                "deploy/staging/Dockerfile",
                "--target",
                "image-decoder",
                "--build-arg",
                f"SOURCE_REVISION={revision}",
                "--tag",
                tag,
                ".",
            ]
        )
        image_built = True
        label = run(
            [
                "docker",
                "image",
                "inspect",
                "--format",
                '{{ index .Config.Labels "org.opencontainers.image.revision" }}',
                tag,
            ],
            capture=True,
        ).stdout.strip()
        if label != revision:
            raise AssertionError("decoder image revision label mismatch")
        run(compose + ["up", "--no-deps", "--detach", "--pull", "never", "image-decoder"], env=env)
        container = run(
            compose + ["ps", "-q", "image-decoder"], env=env, capture=True
        ).stdout.strip()
        if not container:
            raise AssertionError("decoder container was not created")
        path = socket_dir / "decoder.sock"
        deadline = time.monotonic() + 20
        while run(["sudo", "-n", "test", "-S", str(path)], check=False).returncode:
            if time.monotonic() >= deadline:
                raise AssertionError("decoder socket did not appear")
            time.sleep(0.1)
        assert_runtime(container)
        for mode, uid in (("unauthorized", "1002"), ("api", "1000")):
            run(
                [
                    "sudo",
                    "-n",
                    "setpriv",
                    "--reuid",
                    uid,
                    "--regid",
                    "1000",
                    "--clear-groups",
                    "python3",
                    "-",
                    "--client",
                    mode,
                    str(Path(socket_dir.name) / path.name),
                ],
                cwd=temp,
                input_text=client_script,
            )
        print("Unauthorized UID was denied and the isolated container recovered")
        run(compose + ["stop", "-t", "10", "image-decoder"], env=env)
        stopped = json.loads(run(["docker", "inspect", container], capture=True).stdout)[0]["State"]
        removed = run(["sudo", "-n", "test", "!", "-e", str(path)], check=False).returncode == 0
        if stopped["Running"] or stopped["ExitCode"] != 0 or not removed:
            raise AssertionError("decoder failed to exit cleanly and remove its socket")
        print("Graceful stop removed the owned socket and exited successfully")
    finally:
        if container:
            run(["docker", "logs", container], check=False)
        errors = []
        if run(
            compose + ["rm", "--stop", "--force", "image-decoder"], env=env, check=False
        ).returncode:
            errors.append("container")
        if image_built and run(["docker", "image", "rm", tag], check=False).returncode:
            errors.append("image tag")
        if run(["sudo", "-n", "rm", "-rf", "--", str(socket_dir)], check=False).returncode:
            errors.append("socket directory")
        shutil.rmtree(temp)
        if errors:
            raise RuntimeError("could not remove smoke " + ", ".join(errors))


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--client":
        client(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 1:
        main()
    else:
        raise SystemExit("invalid arguments")
