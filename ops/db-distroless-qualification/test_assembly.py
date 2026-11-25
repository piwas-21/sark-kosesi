import gzip
import io
import json
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from unittest.mock import patch

import assembly as recipe


def tar_bytes(entries, *, archive_format=tarfile.PAX_FORMAT, modes=None):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:", format=archive_format) as tar:
        for name, kind, contents in entries:
            info = tarfile.TarInfo(name)
            info.type = kind
            info.mode = (modes or {}).get(name, 0o755 if kind == tarfile.DIRTYPE else 0o644)
            if kind == tarfile.REGTYPE:
                info.size = len(contents)
                tar.addfile(info, io.BytesIO(contents))
            else:
                if kind in {tarfile.SYMTYPE, tarfile.LNKTYPE}:
                    info.linkname = contents
                tar.addfile(info)
    return buffer.getvalue()


def tar_metadata(name, *, mode=0o755, uid=0, pax=None, global_pax=None):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:", pax_headers=global_pax) as tar:
        entry = tarfile.TarInfo(name)
        entry.type, entry.uid, entry.mode = tarfile.DIRTYPE, uid, mode
        entry.pax_headers = pax or {}
        tar.addfile(entry)
    return buffer.getvalue()


class AssemblyTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=recipe.HERE)
        self.addCleanup(directory.cleanup)
        self.folder = Path(directory.name)

    def source(self, extra_layer=(), source_tamper=False, changed_share=False):
        prefix = recipe.PG
        expected = (
            recipe.SERVER_BIN
            | recipe.TOOLS_BIN
            | {
                f"{prefix}/share/postgresql/postgres.bki",
                f"{prefix}/lib/libpq.so.5.18",
                f"{prefix}/lib/postgresql/plpgsql.so",
            }
        )
        entries = [(name, tarfile.REGTYPE, name.encode()) for name in sorted(expected)]
        entries += [
            (f"{prefix}/lib/libpq.so.5", tarfile.SYMTYPE, "libpq.so.5.18"),
            (f"{prefix}/lib/libpq.so", tarfile.SYMTYPE, "libpq.so.5"),
            (f"{prefix}/share/postgresql/timezone/A", tarfile.REGTYPE, b"timezone"),
            (
                f"{prefix}/share/postgresql/timezone/B",
                tarfile.LNKTYPE,
                f"{prefix}/share/postgresql/timezone/A",
            ),
            (f"{prefix}/lib/postgresql/pgxs/build.sh", tarfile.REGTYPE, b"build helper"),
            (f"{prefix}/bin/pg_ctl", tarfile.REGTYPE, b"excluded control tool"),
        ]
        file_values = {name: payload for name, kind, payload in entries if kind == tarfile.REGTYPE}
        for name, kind, payload in entries:
            if kind == tarfile.LNKTYPE:
                file_values[name] = file_values[payload]
        file_rows = [
            f"{recipe.sha_bytes(payload)[7:]}  /{name}"
            for name, payload in sorted(file_values.items())
        ]
        meta_rows = [
            f"644 0 0 /{name} {payload if kind == tarfile.SYMTYPE else ''}"
            for name, kind, payload in entries
        ]
        if changed_share:
            entries = [
                (name, kind, b"changed timezone" if name.endswith("/timezone/A") else payload)
                for name, kind, payload in entries
            ]
        config = json.dumps({"os": "linux", "architecture": "amd64"}).encode()
        layers = [("sources.tar", tarfile.REGTYPE, gzip.compress(tar_bytes(entries)))]
        if extra_layer:
            layers += [("unreviewed.tar", tarfile.REGTYPE, tar_bytes(extra_layer))]
        manifest = [{"Config": "config.json", "Layers": [row[0] for row in layers]}]
        archive = self.folder / "fixture.tar"
        archive.write_bytes(
            tar_bytes(
                [
                    ("manifest.json", tarfile.REGTYPE, json.dumps(manifest).encode()),
                    ("config.json", tarfile.REGTYPE, config),
                    *layers,
                ]
            )
        )
        expected_hash = "f" * 64 if source_tamper else recipe.digest(archive)
        lock = {
            "source_archive_sha256": expected_hash,
            "source_image": recipe.sha_bytes(config),
            "file_sha256": {"/" + p: recipe.sha_bytes(p.encode())[7:] for p in expected},
            "files_sha256": recipe.sha_bytes(("\n".join(file_rows) + "\n").encode())[7:],
            "metadata_sha256": recipe.sha_bytes(("\n".join(sorted(meta_rows)) + "\n").encode())[7:],
        }
        return archive, lock

    def test_sealed_source_split_preserves_data_modules_and_timezones(self):
        archive, lock = self.source()
        server, tools = self.folder / "server.tar", self.folder / "tools.tar"
        files = recipe.app_layers(archive, lock, server, tools)
        for file, wanted, excluded in (
            (server, recipe.SERVER_BIN, recipe.TOOLS_BIN),
            (tools, recipe.TOOLS_BIN, recipe.SERVER_BIN),
        ):
            with tarfile.open(file, "r:") as tar:
                names = set(tar.getnames())
                self.assertTrue(wanted <= names)
                self.assertFalse(excluded & names)
                self.assertFalse(any("pgxs" in name or "pg_ctl" in name for name in names))
        self.assertIn(f"{recipe.PG}/lib/postgresql/plpgsql.so", files)
        self.assertIn(f"{recipe.PG}/share/postgresql/timezone/B", files)

    def test_source_rejects_archive_binaries_and_later_overlay_changes(self):
        outputs = (self.folder / "server.tar", self.folder / "tools.tar")
        archive, lock = self.source(source_tamper=True)
        with self.assertRaisesRegex(ValueError, "source archive differs"):
            recipe.app_layers(archive, lock, *outputs)
        archive, lock = self.source()
        lock["file_sha256"]["/" + recipe.POSTGRES.lstrip("/")] = "f" * 64
        with self.assertRaisesRegex(ValueError, "PostgreSQL payload differs"):
            recipe.app_layers(archive, lock, *outputs)
        archive, lock = self.source(changed_share=True)
        with self.assertRaisesRegex(ValueError, "Exact archived PostgreSQL tree differs"):
            recipe.app_layers(archive, lock, *outputs)
        for name, error in (
            (f"{recipe.PG}/bin/postgres", "Ambiguous source archive member"),
            ("opt/.wh..wh..opq", "masked by a whiteout"),
        ):
            archive, lock = self.source(extra_layer=[(name, tarfile.REGTYPE, b"later")])
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, error):
                recipe.app_layers(archive, lock, *outputs)

    def test_layer_validators_reject_vendor_overwrite_symlink_traversal_and_server_tool(self):
        base = {
            PurePosixPath("usr/lib/ssl.so"): (tarfile.REGTYPE, 0o644, 0, 0),
            PurePosixPath("lib"): (tarfile.SYMTYPE, 0o777, 0, 0),
        }
        for entry, error in (
            (("usr/lib/ssl.so", tarfile.REGTYPE, b"replacement"), "would overwrite"),
            (("lib/moved.so", tarfile.REGTYPE, b"new"), "nondirectory"),
            (("usr/bin/sh", tarfile.SYMTYPE, "bash"), "known tool"),
            (("usr/lib/.wh.ssl.so", tarfile.REGTYPE, b""), "whiteout"),
        ):
            with tarfile.open(fileobj=io.BytesIO(tar_bytes([entry]))) as stream:
                with self.subTest(entry=entry), self.assertRaisesRegex(ValueError, error):
                    recipe.check_layer(stream, base.copy(), shell_free=True)

    def test_normal_debian_header_root_long_names_and_plain_pax_still_pass(self):
        # Ordinary dpkg-deb GNU-format data tar uses ./, named parent dirs, and files.
        filename = "usr/share/doc/" + "long-named-package-" * 8
        payload = tar_bytes(
            [
                ("./", tarfile.DIRTYPE, None),
                ("./usr", tarfile.DIRTYPE, None),
                ("./usr/share", tarfile.DIRTYPE, None),
                ("./" + filename, tarfile.REGTYPE, b"normal Debian payload"),
                ("./usr/manual", tarfile.SYMTYPE, filename),
            ],
            archive_format=tarfile.GNU_FORMAT,
        )
        with tarfile.open(fileobj=io.BytesIO(payload)) as stream:
            paths = recipe.check_layer(stream, {}, shell_free=True)
        self.assertIn(PurePosixPath(filename), paths)
        safe_pax = tar_metadata("long-directory", pax={"mtime": "12.34", "path": "usr/share/test"})
        with tarfile.open(fileobj=io.BytesIO(safe_pax)) as stream:
            self.assertIn(PurePosixPath("usr/share/test"), recipe.check_layer(stream, {}))

    def test_rejects_privileged_root_member_and_security_pax_metadata(self):
        cases = [
            ("root-set-id", tar_metadata("./", mode=0o6755), "Set-ID"),
            ("root-owner", tar_metadata("./", uid=1000), "root metadata"),
            ("member-set-id", tar_metadata("usr/share/test", mode=0o2755), "Set-ID"),
            (
                "global-pax",
                tar_metadata("./", global_pax={"mtime": "12.34"}),
                "Unsupported layer tar record",
            ),
        ]
        for name in ("./", "usr/share/test"):
            for key in (
                "SCHILY.xattr.security.capability",
                "SCHILY.acl.access",
                "SCHILY.mode",
                "RHT.security.selinux",
                "GNU.sparse.major",
            ):
                cases.append(
                    (key, tar_metadata(name, pax={key: "fixture"}), "Unsupported or duplicate PAX")
                )
        for label, data, expected in cases:
            with self.subTest(label=label), self.assertRaisesRegex(ValueError, expected):
                with tarfile.open(fileobj=io.BytesIO(data)) as stream:
                    recipe.check_layer(stream, {})

    def test_rejects_unsupported_raw_types_stacked_metadata_and_hidden_trailer(self):
        kinds = (
            tarfile.GNUTYPE_SPARSE,
            tarfile.CONTTYPE,
            tarfile.CHRTYPE,
            tarfile.BLKTYPE,
            tarfile.FIFOTYPE,
            tarfile.SOLARIS_XHDTYPE,
            b"V",
        )
        ending = bytes(tarfile.BLOCKSIZE * 2)
        for kind in kinds:
            entry = tarfile.TarInfo("unsupported-type")
            entry.type = kind
            data = entry.tobuf(tarfile.GNU_FORMAT) + ending
            with (
                self.subTest(kind=kind),
                self.assertRaisesRegex(ValueError, "Unsupported layer tar record"),
            ):
                recipe.check_tar_headers(io.BytesIO(data))
        safe = tar_metadata("test", pax={"mtime": "12.34"})
        hidden = tar_metadata("test", pax={"SCHILY.xattr.security.capability": "fixture"})
        stacked = safe[: tarfile.BLOCKSIZE * 2] + hidden
        with self.assertRaisesRegex(ValueError, "Unsupported layer extension sequence"):
            recipe.check_tar_headers(io.BytesIO(stacked))
        with self.assertRaisesRegex(ValueError, "Unexpected layer tar trailer"):
            recipe.check_tar_headers(io.BytesIO(safe + hidden))

    def test_offline_assembler_argv_uses_exact_parent_and_distinct_entrypoints(self):
        tool = self.folder / "regctl-linux-amd64"
        layout = self.folder / "images"
        layer = self.folder / "postgres.tar"
        digest = "sha256:" + "a" * 64
        for role, entrypoint in (("server", recipe.POSTGRES), ("tools", recipe.TOOLS_ENTRY)):
            argv = recipe.image_command(tool, layout, digest, role, [layer], role=role)
            self.assertEqual(
                argv[1:6], ["image", "mod", f"ocidir://{layout}@{digest}", "--create", role]
            )
            self.assertEqual(argv[argv.index("--config-entrypoint") + 1], json.dumps([entrypoint]))
            self.assertIn(f"tar={layer}", argv)
            self.assertNotIn("--replace", argv)
        with self.assertRaisesRegex(ValueError, "exact OCI parent"):
            recipe.image_command(tool, layout, "vendor:latest", "server", [layer], role="server")
        package = self.folder / "libstdc++6.tar"
        common = recipe.image_command(tool, layout, digest, "common", [package], role="common")
        self.assertIn(f"tar={package}", common)
        for separator in (",", "="):
            with self.subTest(separator=separator), self.assertRaisesRegex(ValueError, "OCI path"):
                invalid = self.folder / f"package{separator}option.tar"
                recipe.image_command(tool, layout, digest, "common", [invalid], role="common")

    def test_package_layer_uses_unmodified_payload_and_full_vendor_control(self):
        debian = self.folder / "debian-inputs"
        debian.mkdir()
        package = debian / "libtest.deb"
        package.write_bytes(b"pin complete vendor artifact")
        control = b"Package: libtest\nVersion: 3.1\nArchitecture: amd64\nDescription: control stays intact\n"
        payload = tar_bytes([("usr/lib/test.so", tarfile.REGTYPE, b"library payload")])
        record = {
            "libtest": {
                "version": "3.1",
                "arch": "amd64",
                "bytes": package.stat().st_size,
                "sha256": recipe.digest(package),
                "installed_kib": 12,
            }
        }

        def dpkg(args, *, stdout, **kwargs):
            self.assertEqual(args[:2], ["dpkg-deb", "--fsys-tarfile"])
            stdout.write(payload)
            return subprocess.CompletedProcess(args, 0)

        with (
            patch.object(recipe.subprocess, "run", side_effect=dpkg),
            patch.object(recipe.subprocess, "check_output", return_value=control),
        ):
            data, status = recipe.package_layers(
                self.folder, self.folder, record, {}, shell_free=True
            )
        self.assertEqual(data.read_bytes(), payload)
        with tarfile.open(status, "r:") as status_tar:
            self.assertEqual(status_tar.extractfile(recipe.STATUS + "libtest").read(), control)

    def test_oci_receipt_rejects_substituted_payload_even_with_forged_diff_id(self):
        base_layer = {
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": "sha256:" + "f" * 64,
            "size": 100,
        }
        base = {"layers": [base_layer]}
        base_config = {"rootfs": {"diff_ids": ["sha256:" + "b" * 64]}}
        raw = self.folder / "added.tar"
        raw.write_bytes(tar_bytes([("expected", tarfile.REGTYPE, b"payload")]))
        layout = self.folder / "oci"
        blobs = layout / "blobs" / "sha256"
        blobs.mkdir(parents=True)

        def put(data, mime):
            sha = recipe.sha_bytes(data)
            (blobs / sha[7:]).write_bytes(data)
            return {"digest": sha, "size": len(data), "mediaType": mime}

        config = {
            "config": {
                "User": recipe.USER,
                "Entrypoint": [recipe.POSTGRES],
                "Env": ["LANG=en_US.utf8"],
            },
            "rootfs": {
                "diff_ids": [*base_config["rootfs"]["diff_ids"], "sha256:" + recipe.digest(raw)]
            },
        }
        for data in (raw.read_bytes(), b"replaced uncompressed contents"):
            desc = put(gzip.compress(data), base_layer["mediaType"])
            image = {"layers": [base_layer, desc]}
            with patch.object(
                recipe, "selected_image", return_value=({"digest": "test"}, image, config)
            ):
                if data == raw.read_bytes():
                    self.assertEqual(
                        recipe.derived_image(layout, "server", base, base_config, [raw])[1], image
                    )
                else:
                    with self.assertRaisesRegex(ValueError, "blob payload differs"):
                        recipe.derived_image(layout, "server", base, base_config, [raw])

    def vendor_layer_fixture(self):
        path = "usr/lib/os-release"
        layout = self.folder / "images"
        blobs = layout / "blobs" / "sha256"
        blobs.mkdir(parents=True)

        def put(entries, **tar_options):
            data = gzip.compress(tar_bytes(entries, **tar_options))
            sha = recipe.sha_bytes(data)
            (blobs / sha[7:]).write_bytes(data)
            return {"digest": sha, "size": len(data)}

        original = put(
            [(path, tarfile.REGTYPE, b"VERSION=first"), ("etc/file", tarfile.REGTYPE, b"keep")]
        )
        replacement = put([("./" + path, tarfile.REGTYPE, b"VERSION=second")])
        inherited = {
            "path": path,
            "original": {
                "layer": original["digest"],
                "file_sha256": recipe.sha_bytes(b"VERSION=first")[7:],
            },
            "replacement": {
                "layer": replacement["digest"],
                "file_sha256": recipe.sha_bytes(b"VERSION=second")[7:],
            },
        }
        return layout, original, replacement, inherited, put

    def test_exact_vendor_os_release_replacement_preserves_strict_addon_guard(self):
        layout, original, replacement, inherited, _ = self.vendor_layer_fixture()
        paths, _ = recipe.vendor_layers(layout, [original, replacement], inherited)
        self.assertEqual(
            paths,
            {
                PurePosixPath("usr/lib/os-release"): (tarfile.REGTYPE, 0o644, 0, 0),
                PurePosixPath("etc/file"): (tarfile.REGTYPE, 0o644, 0, 0),
            },
        )
        with (
            tarfile.open(recipe.blob(layout, replacement)) as archive,
            self.assertRaisesRegex(ValueError, "would overwrite"),
        ):
            recipe.check_layer(archive, paths)

    def test_vendor_os_release_replacement_requires_pinned_order_and_pair(self):
        layout, original, replacement, inherited, put = self.vendor_layer_fixture()
        third = put([("usr/lib/os-release", tarfile.REGTYPE, b"VERSION=third")])
        for layers, rule, reason in (
            ([original, replacement], None, "would overwrite"),
            ([replacement, original], inherited, "replacement vendor layer order"),
            ([original, original, replacement], inherited, "original vendor layer order"),
            ([original, replacement, replacement], inherited, "replacement vendor layer order"),
            ([original], inherited, "Missing pinned vendor layer replacement"),
            ([original, replacement, third], inherited, "would overwrite"),
            ([original, replacement], {**inherited, "path": "etc/file"}, "replacement rule"),
        ):
            with (
                self.subTest(layers=layers, reason=reason),
                self.assertRaisesRegex(ValueError, reason),
            ):
                recipe.vendor_layers(layout, layers, rule)

    def test_vendor_os_release_replacement_checks_both_payloads_and_raw_layer_rules(self):
        layout, original, replacement, inherited, put = self.vendor_layer_fixture()
        for role in ("original", "replacement"):
            wrong_payload = {**inherited[role], "file_sha256": "f" * 64}
            with (
                self.subTest(role=role),
                self.assertRaisesRegex(ValueError, "type, ownership, mode or payload differs"),
            ):
                recipe.vendor_layers(
                    layout, [original, replacement], {**inherited, role: wrong_payload}
                )

        for entries, reason in (
            ([("etc/new", tarfile.REGTYPE, b"fresh")], "missing or ambiguous"),
            (
                [("usr/lib/os-release", tarfile.SYMTYPE, "etc/file")],
                "type, ownership, mode or payload differs",
            ),
            (
                [
                    ("usr/lib/os-release", tarfile.REGTYPE, b"VERSION=second"),
                    ("etc/file", tarfile.REGTYPE, b"altered"),
                ],
                "would overwrite",
            ),
        ):
            modified = put(entries)
            pin = {**inherited["replacement"], "layer": modified["digest"]}
            with (
                self.subTest(reason=reason),
                self.assertRaisesRegex(ValueError, reason),
            ):
                recipe.vendor_layers(
                    layout, [original, modified], {**inherited, "replacement": pin}
                )

    def test_tools_shell_layer_writes_one_exact_link_and_preserves_shell_free_server(self):
        layout, _, _, _, put = self.vendor_layer_fixture()
        layer = put([("bin", tarfile.SYMTYPE, "usr/bin")], modes={"bin": 0o777})
        server_paths, vendor_bin_target = recipe.vendor_layers(layout, [layer], None)
        self.assertEqual(vendor_bin_target, "usr/bin")
        tools_paths = server_paths.copy()
        bash = put(
            [("usr/bin/bash", tarfile.REGTYPE, b"signed Bash package fixture")],
            modes={"usr/bin/bash": 0o755},
        )
        with tarfile.open(recipe.blob(layout, bash)) as archive:
            recipe.check_layer(archive, tools_paths)
        shell = PurePosixPath("usr/bin/sh")
        output = self.folder / "tools-shell.tar"
        self.assertEqual(recipe.tools_shell_layer(output, tools_paths, vendor_bin_target), output)
        self.assertIn(shell, tools_paths)
        self.assertNotIn(shell, server_paths)
        with tarfile.open(output) as archive:
            (entry,) = archive.getmembers()
            self.assertEqual(
                (entry.name, entry.type, entry.mode, entry.linkname, entry.uid, entry.gid),
                ("usr/bin/sh", tarfile.SYMTYPE, 0o777, "bash", 0, 0),
            )
            with self.assertRaisesRegex(ValueError, "known tool"):
                recipe.check_layer(archive, server_paths, shell_free=True)

    def test_tools_shell_layer_rejects_wrong_vendor_link_missing_bash_and_existing_shell(self):
        mount = PurePosixPath("bin")
        bash = PurePosixPath("usr/bin/bash")
        shell = PurePosixPath("usr/bin/sh")
        paths = {mount: (tarfile.SYMTYPE, 0o777, 0, 0), bash: (tarfile.REGTYPE, 0o755, 0, 0)}
        for index, (inventory, target, reason) in enumerate(
            (
                (paths, "usr/sbin", "does not resolve"),
                ({**paths, mount: (tarfile.DIRTYPE, 0o755, 0, 0)}, "usr/bin", "does not resolve"),
                ({mount: paths[mount]}, "usr/bin", "requires the verified Bash"),
                (
                    {**paths, bash: (tarfile.SYMTYPE, 0o777, 0, 0)},
                    "usr/bin",
                    "requires the verified Bash",
                ),
                ({**paths, shell: (tarfile.REGTYPE, 0o755, 0, 0)}, "usr/bin", "would overwrite"),
            )
        ):
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                recipe.tools_shell_layer(self.folder / f"shell-{index}.tar", inventory, target)

    def test_vendor_layout_requires_exact_index_manifest_config_and_package_path(self):
        layout = self.folder / "images"
        blobs = layout / "blobs" / "sha256"
        blobs.mkdir(parents=True)
        (layout / "oci-layout").write_text('{"imageLayoutVersion":"1.0.0"}')

        def put(data):
            sha = recipe.sha_bytes(data)
            (blobs / sha[7:]).write_bytes(data)
            return {"digest": sha, "size": len(data)}

        status = recipe.STATUS + "vendor-test"
        layer = put(gzip.compress(tar_bytes([(status, tarfile.REGTYPE, b"vendor package record")])))
        config = put(
            json.dumps(
                {
                    "os": "linux",
                    "architecture": "amd64",
                    "rootfs": {"type": "layers", "diff_ids": ["sha256:" + "b" * 64]},
                }
            ).encode()
        )
        manifest = put(
            json.dumps({"schemaVersion": 2, "config": config, "layers": [layer]}).encode()
        )
        vendor_index = self.folder / "vendor-index.json"
        vendor_index.write_text(
            json.dumps(
                {"manifests": [{**manifest, "platform": {"os": "linux", "architecture": "amd64"}}]}
            )
        )
        (layout / "index.json").write_text(
            json.dumps(
                {
                    "manifests": [
                        {**manifest, "annotations": {"org.opencontainers.image.ref.name": "vendor"}}
                    ]
                }
            )
        )
        lock = {
            "index": "sha256:" + recipe.digest(vendor_index),
            "amd64_manifest": manifest["digest"],
            "config": config["digest"],
            "packages": {"vendor-test": {"member": status}},
        }
        self.assertIn(PurePosixPath(status), recipe.vendor_inventory(layout, vendor_index, lock)[3])
        with self.assertRaisesRegex(ValueError, "index bytes differ"):
            recipe.vendor_inventory(layout, vendor_index, {**lock, "index": "sha256:" + "d" * 64})
        (blobs / layer["digest"][7:]).write_bytes(b"changed layer")
        with self.assertRaisesRegex(ValueError, "OCI blob differs"):
            recipe.vendor_inventory(layout, vendor_index, lock)


if __name__ == "__main__":
    unittest.main()
