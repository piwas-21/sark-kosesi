import copy
import hashlib
import io
import json
import lzma
import subprocess
import tarfile
import tempfile
import unittest
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path
from unittest.mock import patch

import custody
import run_lab
import runtime
import source

import packages


def raw_record(values):
    return "".join(f"{key}: {value}\n" for key, value in values.items()).encode()


def sha(content):
    return hashlib.sha256(content).hexdigest()


def valid_signature(signer, seconds=1577836800):
    return f"[GNUPG:] VALIDSIG {signer} 2020-01-01 {seconds} 0 4 0 1 8 00 {signer}\n"


class PackageTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=packages.HERE)
        self.addCleanup(directory.cleanup)
        self.folder = Path(directory.name)
        self.lock = json.loads((packages.HERE / "inputs.json").read_text())
        self.lock["postgres"] = json.loads((packages.HERE / "source-lock.json").read_text())

    def build_apt_fixture(self):
        package = b"synthetic test bytes"
        signer = "1" * 40
        index_path = "main/binary-amd64/Packages.xz"
        control = {"Package": "test-one", "Version": "3.1", "Architecture": "amd64"}
        filename = "pool/main/t/test-one/test-one_3.1_amd64.deb"
        metadata = {**control, "Size": len(package), "SHA256": sha(package), "Filename": filename}
        index = lzma.compress(raw_record(metadata))
        date = "Wed, 01 Jan 2020 00:00:00 UTC"
        release = f"Codename: trixie\nDate: {date}\nSHA256:\n {sha(index)} {len(index)} {index_path}\n".encode()
        keyring = b"synthetic keyring fixture"
        recipe = {
            "version": control["Version"],
            "arch": control["Architecture"],
            "filename": filename,
            "bytes": len(package),
            "sha256": sha(package),
        }
        values = {
            "debian": {
                "suite": "trixie",
                "index_path": index_path,
                "index_sha256": sha(index),
                "index_bytes": len(index),
                "keyring_sha256": sha(keyring),
                "release_sha256": sha(release),
                "signature_sha256": sha(b"synthetic signature"),
                "release_date": date,
                "snapshot_time": "2020-01-02T00:00:00+00:00",
                "valid_until": None,
                "signers": [signer],
                "packages": {"test-one": recipe},
            }
        }
        for name, data in (
            ("Release", release),
            ("Release.gpg", b"synthetic signature"),
            ("Packages.xz", index),
            ("debian-archive-keyring.gpg", keyring),
            ("test-one.deb", package),
        ):
            (self.folder / name).write_bytes(data)
        return values, signer, control

    def test_valid_signed_index_and_payload_chain(self):
        values, signer, control = self.build_apt_fixture()
        with patch.object(packages.subprocess, "run") as run:
            run.side_effect = [
                subprocess.CompletedProcess([], 0, valid_signature(signer)),
                subprocess.CompletedProcess([], 0, raw_record(control)),
            ]
            result = packages.package_inputs(self.folder, values)
            self.assertEqual(result["approved_signers"], [signer])
            self.assertEqual(result["signature_times"], [1577836800])
            self.assertEqual(result["package_count"], 1)
            self.assertEqual([call.args[0][0] for call in run.call_args_list], ["gpgv", "dpkg-deb"])
            self.assertTrue(all(call.kwargs["check"] for call in run.call_args_list))

    def test_wrong_signer_or_tampered_payload_never_reaches_dpkg(self):
        values, signer, _control = self.build_apt_fixture()
        with patch.object(packages.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, valid_signature("f" * 40))
            with self.assertRaisesRegex(ValueError, "reviewed Debian archive signer"):
                packages.package_inputs(self.folder, values)
            self.assertEqual(run.call_count, 1)
            run.reset_mock()
            (self.folder / "test-one.deb").write_bytes(b"synthetic test byteS")
            run.return_value = subprocess.CompletedProcess([], 0, valid_signature(signer))
            with self.assertRaisesRegex(ValueError, "Package bytes differ"):
                packages.package_inputs(self.folder, values)
            self.assertEqual(run.call_count, 1)

    def test_frozen_signing_window_signature_bytes_and_valid_until_are_enforced(self):
        values, signer, _ = self.build_apt_fixture()
        with patch.object(packages.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess(
                [], 0, valid_signature(signer, 1577836799)
            )
            with self.assertRaisesRegex(ValueError, "signature time"):
                packages.package_inputs(self.folder, values)
            self.assertEqual(run.call_count, 1)
            run.reset_mock()
            (self.folder / "Release.gpg").write_bytes(b"changed signature bytes")
            with self.assertRaisesRegex(ValueError, "signature pin"):
                packages.package_inputs(self.folder, values)
            run.assert_not_called()
        expiry = "Fri, 03 Jan 2020 00:00:00 UTC"
        feed = (self.folder / "Release").read_text() + f"Valid-Until: {expiry}\n"
        policy = {**values["debian"], "valid_until": expiry}
        self.assertEqual(
            packages.release_window(feed, policy, datetime(2020, 1, 2, tzinfo=UTC)),
            (1577836800, 1577923200),
        )
        with self.assertRaisesRegex(ValueError, "Valid-Until has passed"):
            packages.release_window(feed, policy, datetime(2020, 1, 4, tzinfo=UTC))
        with self.assertRaisesRegex(ValueError, "Valid-Until differs"):
            packages.release_window(feed, values["debian"], datetime(2020, 1, 2, tzinfo=UTC))

    def test_owner_only_scratch_and_fresh_receipt_override_rejected(self):
        manifest = packages.HERE / "source-lock.json"
        lock = json.loads(manifest.read_text())
        ext = {"source_lock_sha256": sha(manifest.read_bytes())}
        receipt = {
            key: lock[key] for key in ("artifact_id", "upstream_sha256", *source.SOURCE_FIELDS)
        }
        receipt.update(source_image="sha256:" + "a" * 64, source_archive_sha256="b" * 64)
        (self.folder / "source-receipt.json").write_text(json.dumps(receipt))
        self.assertEqual(source.bound_source(self.folder, ext)["file_sha256"], lock["file_sha256"])
        (self.folder / "source-receipt.json").write_text(json.dumps({**receipt, "file_sha256": {}}))
        with self.assertRaisesRegex(ValueError, "Unexpected fresh source receipt fields"):
            source.bound_source(self.folder, ext)
        owned = self.folder / "owned"
        owned.mkdir(mode=0o700)
        wrong = self.folder / "debian-inputs"
        wrong.symlink_to(owned, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "Unexpected owner or path"):
            packages.owned_directory(wrong)

    def rootfs_fixture(self):
        records = self.lock["base"]["packages"] | self.lock["debian"]["packages"]
        files = {
            packages.STATUS + name: raw_record(
                {
                    "Package": name,
                    "Version": meta["version"],
                    "Architecture": meta["arch"],
                }
            )
            for name, meta in records.items()
        }
        for meta in self.lock["base"]["packages"].values():
            member = meta["member"] + ".md5sums"
            files[member] = ("synthetic " + member).encode()
            meta["md5sums_bytes"] = len(files[member])
            meta["md5sums_sha256"] = sha(files[member])
        for name in packages.SERVER_FILES:
            files[name] = ("synthetic " + name).encode()
            self.lock["postgres"]["file_sha256"]["/" + name] = sha(files[name])
        return files

    def write_rootfs(self, files):
        path = self.folder / "server.tar"
        with tarfile.open(path, "w") as output:
            for member, content in files.items():
                entry = tarfile.TarInfo(member)
                if content is None:
                    entry.type = tarfile.SYMTYPE
                    entry.linkname = "usr/bin/dash"
                    output.addfile(entry)
                else:
                    entry.size = len(content)
                    output.addfile(entry, io.BytesIO(content))
        return path

    def test_status_d_inventory_preserves_all_packages_and_known_server_hashes(self):
        files = self.rootfs_fixture()
        output = packages.image_inventory(self.write_rootfs(files), self.lock)
        self.assertEqual(len(output), 16)
        self.assertIn("libssl3t64\t3.5.9-1~deb13u1\tinstall ok installed", output)
        self.assertIn("locales-all\t2.41-12+deb13u5\tinstall ok installed", output)

    def test_inventory_rejects_tools_extra_packages_and_changed_server(self):
        files = self.rootfs_fixture()
        additions = (
            ("usr/bin/sh", None, "bootstrap or client tools"),
            ("opt/pg-smoke/bin/psql", b"tool", "bootstrap or client tools"),
            (
                packages.STATUS + "libsqlite3-0",
                raw_record(
                    {
                        "Package": "libsqlite3-0",
                        "Version": "4",
                        "Architecture": "amd64",
                    }
                ),
                "Unexpected dpkg status.d package path",
            ),
            (packages.SERVER_FILES[0], b"different server", "Measured PostgreSQL file differs"),
            (
                packages.STATUS + "libssl3t64",
                b"Package: libssl3t64\nPackage: forged\n",
                "duplicate Debian field",
            ),
        )
        for name, contents, error in additions:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, error):
                packages.image_inventory(self.write_rootfs({**files, name: contents}), self.lock)
        native_member = packages.STATUS + "libssl3t64"
        files[packages.STATUS + "moved-libssl3t64"] = files.pop(native_member)
        with self.assertRaisesRegex(ValueError, "Unexpected dpkg status.d package path"):
            packages.image_inventory(self.write_rootfs(files), self.lock)

    def test_inventory_requires_exact_vendor_checksum_lists(self):
        files = self.rootfs_fixture()
        checksum = packages.STATUS + "libssl3t64.md5sums"
        for name, payload in (
            (checksum, b"x" * len(files[checksum])),
            (checksum, None),
            (packages.STATUS + "unknown.md5sums", files[checksum]),
        ):
            with (
                self.subTest(name=name, payload=payload),
                self.assertRaisesRegex(ValueError, "Unexpected dpkg status.d checksum list"),
            ):
                packages.image_inventory(self.write_rootfs({**files, name: payload}), self.lock)
        del files[checksum]
        with self.assertRaisesRegex(ValueError, "Server package or PostgreSQL inventory differs"):
            packages.image_inventory(self.write_rootfs(files), self.lock)


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.scope = runtime.Scope(
            "a" * 24, "sha256:" + "b" * 64, "sha256:" + "c" * 64, "approved-lab"
        )
        self.server_id = "d" * 64
        self.id = "e" * 64
        self.volume = {
            "Name": self.scope.volume,
            "Driver": "local",
            "Scope": "local",
            "Labels": {runtime.LABEL: self.scope.nonce},
            "Options": None,
        }

    def record(self, role):
        scope = self.scope
        image, network, entry, command = runtime.role_parameters(scope, role, self.server_id)
        return {
            "Id": self.server_id if role == "server" else self.id,
            "Image": image,
            "Name": "/" + scope.name(role),
            "Config": {
                "Labels": {runtime.LABEL: scope.nonce},
                "User": "0:0" if role == "prepare" else runtime.USER,
                "Entrypoint": [entry],
                "Cmd": command,
                "StopSignal": "SIGINT",
            },
            "HostConfig": {
                "NetworkMode": network,
                "IpcMode": "private",
                "ReadonlyRootfs": True,
                "Privileged": False,
                "Memory": 512 * 1024**2,
                "MemorySwap": 512 * 1024**2,
                "NanoCpus": 10**9,
                "PidsLimit": 128,
                "Tmpfs": {"/tmp": runtime.TMPFS},
                "Mounts": []
                if role == "client"
                else [
                    {
                        "Type": "volume",
                        "Source": scope.volume,
                        "Target": runtime.VOLUME_TARGET,
                        "VolumeOptions": {"NoCopy": True},
                    }
                ],
                "CapDrop": ["ALL"],
                "CapAdd": ["CAP_CHOWN"]
                if role == "prepare" and scope.data_owner == runtime.USER
                else None,
                "SecurityOpt": ["no-new-privileges"],
            },
            "Mounts": []
            if role == "client"
            else [
                {
                    "Type": "volume",
                    "Name": scope.volume,
                    "Destination": runtime.VOLUME_TARGET,
                    "RW": True,
                }
            ],
            "NetworkSettings": {"Ports": {}},
            "State": {"Running": True, "OOMKilled": False, "Pid": 321},
        }

    def test_owned_split_roles_without_routes_and_direct_nonroot_pid_one(self):
        for role in ("prepare", "server", "bootstrap", "client"):
            record = self.record(role)
            command = runtime.create_args(
                self.scope,
                role,
                daemon_id=self.scope.daemon,
                volume=self.volume,
                server_id=self.server_id,
                server=self.record("server"),
            )
            self.assertNotIn("--publish", command)
            self.assertNotIn("--privileged", command)
            runtime.container_owned(record, self.scope, role, record["Id"], self.server_id)
            if role == "client":
                self.assertNotIn("--mount", command)
        runtime.direct_server(self.record("server"), self.scope, self.server_id, 321, "postgres\n")

    def test_only_normal_prepare_gets_the_exact_docker_chown_capability(self):
        for data_owner, capability in ((runtime.USER, ["CAP_CHOWN"]), ("0:0", None)):
            self.scope = replace(self.scope, data_owner=data_owner)
            record = self.record("prepare")
            command = runtime.create_args(
                self.scope, "prepare", daemon_id=self.scope.daemon, volume=self.volume
            )
            if capability:
                self.assertEqual(command[command.index("--cap-add") + 1], "CAP_CHOWN")
            else:
                self.assertNotIn("--cap-add", command)
            runtime.container_owned(record, self.scope, "prepare", self.id)
            record["HostConfig"]["CapAdd"] = [*(capability or []), "CAP_DAC_OVERRIDE"]
            with self.assertRaisesRegex(ValueError, "unverified"):
                runtime.container_owned(record, self.scope, "prepare", self.id)

    def test_isolation_and_package_role_guards_fail_closed(self):
        for mapping, key, value in (
            ("HostConfig", "PortBindings", {"5432/tcp": [{"HostIp": "127.0.0.1"}]}),
            ("HostConfig", "NetworkMode", "bridge"),
            ("HostConfig", "MemorySwap", -1),
            ("Config", "User", "0:0"),
            ("Config", "Labels", {runtime.LABEL: "foreign"}),
            ("Config", "Entrypoint", ["/bin/sh"]),
        ):
            actual = copy.deepcopy(self.record("server"))
            actual[mapping][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "unverified"):
                runtime.container_owned(actual, self.scope, "server", self.server_id)
        with self.assertRaisesRegex(ValueError, "unverified"):
            runtime.container_owned(self.record("client"), self.scope, "client", self.id, "f" * 64)
        with self.assertRaisesRegex(ValueError, "PID 1"):
            runtime.direct_server(
                self.record("server"), self.scope, self.server_id, 322, "postgres"
            )
        with self.assertRaisesRegex(ValueError, "PID 1"):
            runtime.direct_server(self.record("server"), self.scope, self.server_id, 321, "sh")

    def test_rejects_relaxed_namespace_device_capability_and_security_options(self):
        relaxed = (
            ("PidMode", "host"),
            ("PidMode", "container:" + self.id),
            ("IpcMode", "host"),
            ("IpcMode", "shareable"),
            ("UTSMode", "host"),
            ("UsernsMode", "host"),
            ("Devices", [{"PathOnHost": "/dev/random"}]),
            ("DeviceRequests", [{"Driver": "example"}]),
            ("DeviceCgroupRules", ["a *:* rwm"]),
            ("CapAdd", ["SYS_ADMIN"]),
            ("GroupAdd", ["0"]),
            ("SecurityOpt", ["no-new-privileges", "seccomp=unconfined"]),
            ("SecurityOpt", ["no-new-privileges", "apparmor=unconfined"]),
            ("SecurityOpt", ["no-new-privileges", "label=disable"]),
            ("SecurityOpt", ["no-new-privileges:false"]),
        )
        for role in ("server", "bootstrap", "client"):
            for key, value in relaxed:
                record = self.record(role)
                record["HostConfig"][key] = value
                with (
                    self.subTest(role=role, option=key, value=value),
                    self.assertRaisesRegex(ValueError, "unverified"),
                ):
                    runtime.container_owned(record, self.scope, role, record["Id"], self.server_id)
            record = self.record(role)
            record["HostConfig"]["SecurityOpt"] = ["no-new-privileges:true"]
            runtime.container_owned(record, self.scope, role, record["Id"], self.server_id)

    def test_client_cannot_mount_data_and_volume_requires_own_label(self):
        client = self.record("client")
        client["Mounts"] = self.record("server")["Mounts"]
        with self.assertRaisesRegex(ValueError, "mounts differ"):
            runtime.container_owned(client, self.scope, "client", self.id, self.server_id)
        server = self.record("server")
        server["HostConfig"]["Mounts"][0]["VolumeOptions"]["Subpath"] = "foreign-path"
        with self.assertRaisesRegex(ValueError, "volume options differ"):
            runtime.container_owned(server, self.scope, "server", self.server_id)
        runtime.volume_owned(self.volume, self.scope)
        self.volume["Name"] = "foreign-volume"
        with self.assertRaisesRegex(ValueError, "unverified"):
            runtime.volume_owned(self.volume, self.scope)

    def test_volume_nocopy_accepts_docker_empty_driver_config_in_use_and_cleanup(self):
        server = self.record("server")
        mount = server["HostConfig"]["Mounts"][0]
        mount["VolumeOptions"] = {"NoCopy": True, "DriverConfig": {}}
        runtime.container_owned(server, self.scope, "server", self.server_id)
        server["State"]["Running"] = False
        self.assertEqual(
            runtime.container_remove_args(
                self.scope,
                "server",
                self.server_id,
                server,
                daemon_id=self.scope.daemon,
                volume=self.volume,
            ),
            ["container", "rm", self.server_id],
        )
        for options in (
            {"NoCopy": False, "DriverConfig": {}},
            {"NoCopy": 1, "DriverConfig": {}},
            {"NoCopy": True, "DriverConfig": {"Name": "other"}},
            {"NoCopy": True, "DriverConfig": {"Options": {"type": "tmpfs"}}},
            {"NoCopy": True, "DriverConfig": {}, "Subpath": "other"},
            {"NoCopy": True, "DriverConfig": {}, "Labels": {"conflict": "foreign"}},
        ):
            mount["VolumeOptions"] = options
            with (
                self.subTest(options=options),
                self.assertRaisesRegex(ValueError, "options differ"),
            ):
                runtime.container_owned(server, self.scope, "server", self.server_id)

    def test_client_and_cleanup_require_pinned_daemon_owned_volume_and_exact_server(self):
        client = self.record("client")
        proof = {
            "daemon_id": self.scope.daemon,
            "volume": self.volume,
            "server_id": self.server_id,
            "server": self.record("server"),
        }
        self.assertIn(
            "container:" + self.server_id, runtime.create_args(self.scope, "client", **proof)
        )
        client["State"]["Running"] = False
        remove = runtime.container_remove_args
        self.assertEqual(
            remove(self.scope, "client", self.id, client, **proof), ["container", "rm", self.id]
        )
        for mismatched in (
            {"daemon_id": "different-daemon"},
            {"volume": {**self.volume, "Labels": {runtime.LABEL: "foreign"}}},
            {"server": None},
            {"server_id": "f" * 64},
            {"server": {**proof["server"], "Id": "f" * 64}},
        ):
            arguments = {**proof, **mismatched}
            with self.subTest(changes=mismatched.keys()), self.assertRaises(ValueError):
                runtime.create_args(self.scope, "client", **arguments)
            with self.assertRaises(ValueError):
                remove(self.scope, "client", self.id, client, **arguments)
        client["State"]["Running"] = True
        with self.assertRaisesRegex(ValueError, "Stop the exact owned container"):
            remove(self.scope, "client", self.id, client, **proof)
        with self.assertRaisesRegex(ValueError, "daemon identity differs"):
            runtime.volume_remove_args(self.scope, "different-daemon", self.volume)
        self.assertEqual(
            runtime.volume_remove_args(self.scope, self.scope.daemon, self.volume),
            ["volume", "rm", self.scope.volume],
        )

    def test_all_seven_tls_controls_are_required(self):
        runtime.tls_verdict("\n".join(runtime.TLS_RESULTS) + "\n")
        with self.assertRaisesRegex(ValueError, "Incomplete or failed"):
            runtime.tls_verdict("\n".join(runtime.TLS_RESULTS[:-1]))

    def test_lost_create_response_recovers_only_exact_owned_resource(self):
        lab = run_lab.Lab(self.scope)
        lab.created["server"] = None
        owned = self.record("server")
        foreign = copy.deepcopy(owned)
        foreign["Config"]["Labels"][runtime.LABEL] = "foreign"
        with (
            patch.object(run_lab, "daemon", return_value=self.scope.daemon),
            patch.object(run_lab, "inspect", return_value=foreign) as inspect,
            patch.object(run_lab, "docker") as docker,
        ):
            with self.assertRaisesRegex(ValueError, "unverified"):
                lab.remove("server")
            inspect.assert_called_once_with("container", self.scope.name("server"))
            docker.assert_not_called()
            self.assertIsNone(lab.created["server"])
            inspect.return_value = owned
            self.assertEqual(lab.record("server")["Id"], self.server_id)
            self.assertEqual(lab.created["server"], self.server_id)

    def test_inspect_only_accepts_exact_daemon_missing_diagnostic(self):
        name = self.scope.volume
        with patch.object(run_lab, "docker") as docker:
            docker.return_value = subprocess.CompletedProcess(
                [], 1, b"[]", f"Error response from daemon: get {name}: no such volume\n".encode()
            )
            self.assertIsNone(run_lab.inspect("volume", name))
            docker.return_value = subprocess.CompletedProcess(
                [], 1, b"[]", b"Error response from daemon: permission denied\n"
            )
            with self.assertRaisesRegex(ValueError, "Unexpected Docker inspect failure"):
                run_lab.inspect("volume", name)

    def test_top_level_lost_source_build_response_requires_marker_and_exact_tag(self):
        tag = "local/db-distroless-source:" + self.scope.nonce
        record = {
            "Id": self.scope.server,
            "Os": "linux",
            "Architecture": "amd64",
            "RepoTags": [tag],
            "Config": {"Labels": {custody.SOURCE_MARKER: self.scope.nonce}},
        }
        with patch.object(custody, "inspect", return_value=record):
            self.assertEqual(
                custody.resolve("image", tag, nonce=self.scope.nonce), self.scope.server
            )
            with self.assertRaisesRegex(ValueError, "source build label"):
                custody.resolve("image", tag, nonce="foreign")
            with self.assertRaisesRegex(ValueError, "tagged lab image"):
                custody.resolve("image", "local/wrong-tag", nonce=self.scope.nonce)
            with self.assertRaisesRegex(ValueError, "candidate image ID"):
                custody.resolve("image", tag, expected=self.scope.tools)


class RuntimeDiagnosticsTests(unittest.TestCase):
    def test_failure_sites_identify_both_exceptions_without_rendering_private_text(self):
        diagnostic = b"private runtime diagnostic"
        response = subprocess.CompletedProcess([], 2, b"", diagnostic)
        with patch.object(run_lab, "docker", return_value=response):
            try:
                try:
                    run_lab.inspect("container", "synthetic")
                finally:
                    run_lab.inspect("volume", "synthetic")
            except ValueError as error:
                sites = list(run_lab.failure_sites(error))
            else:
                self.fail("Private error fixture unexpectedly passed")
        self.assertEqual([name for name, _ in sites], ["run_lab.py", "run_lab.py"])
        self.assertTrue(all(isinstance(line, int) and line > 0 for _, line in sites))
        self.assertNotIn(diagnostic.decode(), repr(sites))


class FixtureCustodyTests(unittest.TestCase):
    def test_tls_scripts_gate_directory_permissions_and_disable_xtrace_before_credentials(self):
        work_guard = """[[ "$(stat -c '%u:%g:%a' "$work")" == '65532:65532:700' ]] || exit 1"""
        with tempfile.TemporaryDirectory(dir=packages.HERE) as directory:
            owned = Path(directory) / "owned"
            owned.mkdir(mode=0o700)
            alias = Path(directory) / "alias"
            alias.symlink_to(owned, target_is_directory=True)
            for name in ("bootstrap.sh", "tls-client.sh"):
                script = (packages.HERE / name).read_text()
                prelude, matched, rest = script.partition(work_guard)
                self.assertEqual(matched, work_guard)
                self.assertNotIn('"$work/password"', prelude)
                self.assertIn('"$work/password"', rest)
                probe = (
                    """fixture_stat="$2"
id() { printf '%s\\n' 65532; }
stat() { printf '%s\\n' "$fixture_stat"; }
"""
                    + prelude.replace("work=/tmp/sibling-tls", 'work="$1"')
                    + work_guard
                    + "\n[[ $- != *x* ]]\ntest_marker=synthetic_fixture_value; printf 'reached\\n'\n"
                )
                for path, stat_result, expected in (
                    (owned, "65532:65532:700", True),
                    (owned, "65532:65532:755", False),
                    (owned, "0:0:700", False),
                    (alias, "65532:65532:700", False),
                ):
                    with self.subTest(script=name, path=path.name, metadata=stat_result):
                        result = subprocess.run(
                            [
                                "/bin/bash",
                                "--noprofile",
                                "--norc",
                                "-p",
                                "-x",
                                "-c",
                                probe,
                                "test-fixture",
                                str(path),
                                stat_result,
                            ],
                            capture_output=True,
                            text=True,
                            check=False,
                        )
                        self.assertEqual(result.returncode == 0, expected, result.stderr)
                        self.assertEqual(result.stdout, "reached\n" if expected else "")
                        self.assertNotIn("synthetic_fixture_value", result.stderr)


if __name__ == "__main__":
    unittest.main()
