#!/usr/bin/env python3
"""Assemble two local AMD64 OCI variants from verified vendor and source inputs."""

import argparse
import gzip
import hashlib
import io
import json
import os
import posixpath
import re
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path, PurePosixPath

from runtime import POSTGRES, USER

from packages import (
    BUDGET,
    HERE,
    STATUS,
    TOOL_ONLY,
    digest,
    only_record,
    owned_directory,
    package_inputs,
    regular,
    require,
)

PG = "opt/pg-smoke"
SERVER_BIN = {f"{PG}/bin/postgres"}
TOOLS_BIN = {
    f"{PG}/bin/{name}" for name in ("initdb", "pg_config", "pg_dump", "pg_restore", "psql")
}
TOOLS_ENTRY = "/usr/bin/sleep"
BASE_RESERVE = 16 * 1024**2
PG_BUDGET = 128 * 1024**2
TAR_MEMBERS = {tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE, tarfile.SYMTYPE, tarfile.LNKTYPE}
GNU_NAMES = {tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK}
TAR_EXTENSIONS = GNU_NAMES | {tarfile.XHDTYPE}
PAX_PATHS = {b"path", b"linkpath"}
PAX_TIMES = {b"mtime", b"atime", b"ctime"}
PAX_FIELDS = {name.decode("ascii") for name in PAX_PATHS | PAX_TIMES}
PAX_LENGTH = re.compile(rb"([1-9][0-9]{0,5}) ")
MAX_TAR_EXTENSION = 64 * 1024


def read_json(path, limit=2 * 1024**2):
    return json.loads(regular(path, limit).read_bytes())


def scoped_source(inputs_path, expected):
    require(digest(regular(inputs_path, 64 * 1024)) == expected, "Reviewed lock digest differs")
    lock = read_json(inputs_path, 64 * 1024)
    require(lock["schema"] == 1, "Unexpected lock schema")
    return lock


def sha_bytes(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def name_in_tar(raw):
    path = PurePosixPath(raw)
    require(
        not path.is_absolute() and ".." not in path.parts and "\\" not in raw, "Unexpected tar path"
    )
    require(not any(part.startswith(".wh.") for part in path.parts), "Unexpected whiteout")
    return path


def check_pax(data):
    seen = set()
    position = 0
    while position < len(data):
        length = PAX_LENGTH.match(data, position)
        require(length is not None, "Malformed PAX extension")
        end = position + int(length[1])
        require(length.end() < end <= len(data) and data[end - 1] == 10, "Malformed PAX extension")
        key, equals, value = data[length.end() : end - 1].partition(b"=")
        require(
            equals and key not in seen and key in PAX_PATHS | PAX_TIMES,
            "Unsupported or duplicate PAX field",
        )
        if key in PAX_PATHS:
            require(
                value and all(byte >= 32 and byte != 127 for byte in value), "Malformed PAX path"
            )
            value.decode("utf-8")
        else:
            require(
                re.fullmatch(rb"-?[0-9]{1,19}(\.[0-9]{1,19})?", value), "Malformed PAX timestamp"
            )
        seen.add(key)
        position = end


def check_tar_headers(stream):
    # TarFile folds local and global extensions into logical members. Inspect every
    # on-disk type and PAX record separately before trusting those member objects.
    block = tarfile.BLOCKSIZE
    pending = set()
    for _ in range(100_000):
        header = stream.read(block)
        require(
            len(header) == block and stream.tell() <= BUDGET, "Incomplete or oversized layer tar"
        )
        if header == bytes(block):
            require(not pending and stream.read(block) == bytes(block), "Invalid end of layer tar")
            while trailing := stream.read(1024**2):
                require(
                    not trailing.strip(b"\0") and stream.tell() <= BUDGET,
                    "Unexpected layer tar trailer",
                )
            return
        kind = header[156:157]
        require(kind in TAR_MEMBERS | TAR_EXTENSIONS, "Unsupported layer tar record")
        member = tarfile.TarInfo.frombuf(header, "utf-8", "strict")
        require(not (member.mode & 0o6000), "Set-ID layer member")
        size = member.size
        padded = (size + block - 1) // block * block
        require(0 <= size and stream.tell() + padded <= BUDGET, "Oversized layer tar member")
        if kind in TAR_EXTENSIONS:
            require(
                0 < size <= MAX_TAR_EXTENSION
                and kind not in pending
                and (not pending or pending | {kind} <= GNU_NAMES),
                "Unsupported layer extension sequence",
            )
            extension = stream.read(padded)
            require(
                len(extension) == padded and not extension[size:].strip(b"\0"),
                "Malformed layer extension padding",
            )
            if kind == tarfile.XHDTYPE:
                check_pax(extension[:size])
            else:
                name = extension[: size - 1]
                require(
                    extension[size - 1] == 0
                    and name
                    and all(byte >= 32 and byte != 127 for byte in name),
                    "Malformed GNU tar name",
                )
                name.decode("utf-8")
            pending.add(kind)
            continue
        require(
            kind in {tarfile.REGTYPE, tarfile.AREGTYPE} or size == 0, "Non-file layer tar payload"
        )
        stream.seek(padded, os.SEEK_CUR)
        pending.clear()
    raise ValueError("Too many layer tar records")


def check_layer(tar, underlying, *, shell_free=False):
    start = tar.fileobj.tell()
    try:
        tar.fileobj.seek(0)
        check_tar_headers(tar.fileobj)
    finally:
        tar.fileobj.seek(start)
    observed = {}
    for index, member in enumerate(tar):
        path = name_in_tar(member.name)
        require(index < 100_000 and path not in observed, "Oversized or duplicate layer member")
        require(
            member.type in TAR_MEMBERS and not member.issparse(),
            "Unexpected layer inode",
        )
        require(member.pax_headers.keys() <= PAX_FIELDS, "Unsupported effective PAX metadata")
        require(not (member.mode & 0o6000), "Set-ID layer member")
        if str(path) == ".":
            require(
                member.isdir() and member.uid == member.gid == 0 and member.mode == 0o755,
                "Unexpected layer root metadata",
            )
        if shell_free and str(path.parent) in {"bin", "usr/bin", f"{PG}/bin"}:
            require(path.name not in TOOL_ONLY, "Server layer contains a known tool")
        inode = (member.type, member.mode, member.uid, member.gid)
        if path in underlying:
            require(
                member.isdir() and underlying[path] == inode,
                "Layer would overwrite existing vendor path",
            )
        if member.islnk():
            link = name_in_tar(member.linkname)
            target = observed.get(link) or underlying.get(link)
            require(
                target is not None and target[0] in {tarfile.REGTYPE, tarfile.AREGTYPE},
                "Unexpected hardlink",
            )
        observed[path] = inode
    for path in observed:
        for parent in path.parents:
            inode = observed.get(parent) or underlying.get(parent)
            require(inode is None or inode[0] == tarfile.DIRTYPE, "Layer traverses a nondirectory")
    underlying.update(observed)
    return observed


def blob(layout, desc):
    value = desc["digest"]
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", value), "Malformed OCI blob digest")
    path = regular(layout / "blobs" / "sha256" / value[7:], BUDGET)
    require(
        path.stat().st_size == desc["size"] and "sha256:" + digest(path) == value,
        "OCI blob differs",
    )
    return path


def selected_image(layout, tag, *, expected=None):
    layout_meta = read_json(layout / "oci-layout")
    require(layout_meta["imageLayoutVersion"] == "1.0.0", "Unexpected OCI layout version")
    index = read_json(layout / "index.json")
    rows = [
        d
        for d in index["manifests"]
        if d.get("annotations", {}).get("org.opencontainers.image.ref.name") == tag
    ]
    require(len(rows) == 1, "Missing or ambiguous local OCI image")
    desc = rows[0]
    require(expected is None or desc["digest"] == expected, "OCI manifest identity differs")
    manifest = read_json(blob(layout, desc))
    config = read_json(blob(layout, manifest["config"]))
    require(
        manifest["schemaVersion"] == 2
        and config["os"] == "linux"
        and config["architecture"] == "amd64"
        and config["rootfs"]["type"] == "layers"
        and 0 < len(manifest["layers"]) == len(config["rootfs"]["diff_ids"]) <= 64,
        "Unexpected OCI platform or layer inventory",
    )
    return desc, manifest, config


def vendor_inventory(layout, raw_index, lock):
    require(
        "sha256:" + digest(regular(raw_index, 2 * 1024**2)) == lock["index"],
        "Signed index bytes differ",
    )
    index = read_json(raw_index)
    amd64 = [
        row
        for row in index["manifests"]
        if row.get("platform") == {"os": "linux", "architecture": "amd64"}
    ]
    require(
        len(amd64) == 1 and amd64[0]["digest"] == lock["amd64_manifest"],
        "Signed index AMD64 descriptor differs",
    )
    desc, manifest, config = selected_image(layout, "vendor", expected=lock["amd64_manifest"])
    local_index = read_json(layout / "index.json")
    require(len(local_index["manifests"]) == 1, "Run requires a new vendor-only OCI layout")
    require(
        desc["size"] == amd64[0]["size"] and manifest["config"]["digest"] == lock["config"],
        "Vendor AMD64 config or size differs",
    )
    paths = {}
    for layer in manifest["layers"]:
        with tarfile.open(blob(layout, layer), "r:*") as tar:
            check_layer(tar, paths, shell_free=True)
    for record in lock["packages"].values():
        require(PurePosixPath(record["member"]) in paths, "Expected vendor package path missing")
    return desc, manifest, config, paths


def source_binary(path):
    return str(path) in SERVER_BIN | TOOLS_BIN


def source_payload(path, member):
    folders = path.parts
    if folders[:3] == ("opt", "pg-smoke", "share"):
        require(not member.mode & 0o111 or not member.isfile(), "Executable in server shared data")
        return True
    return (
        path.parent in {PurePosixPath(PG, "lib"), PurePosixPath(PG, "lib", "postgresql")}
        and re.fullmatch(r".+\.so(?:\.[0-9]+)*", path.name) is not None
    )


def app_layers(archive, pg, server_tar, tools_tar):
    require(
        digest(regular(archive, BUDGET)) == pg["source_archive_sha256"],
        "Freshly sealed source archive differs",
    )
    found = {}
    symlinks = {}
    all_files = {}
    all_metadata = {}
    budget = PG_BUDGET
    tree_count = 0
    with tarfile.open(archive, "r:") as outer:
        (row,) = json.load(outer.extractfile("manifest.json"))
        config_raw = outer.extractfile(row["Config"]).read(2 * 1024**2)
        require(
            sha_bytes(config_raw) == pg["source_image"],
            "Measured source image ID differs",
        )
        cfg = json.loads(config_raw)
        require(
            (cfg["os"], cfg["architecture"]) == ("linux", "amd64"),
            "Measured source platform differs",
        )
        require(0 < len(row["Layers"]) <= 128, "Unexpected source layer count")
        with tarfile.open(server_tar, "w:") as server, tarfile.open(tools_tar, "w:") as tools:
            for name in row["Layers"]:
                layer_contains_tree = False
                with tarfile.open(fileobj=outer.extractfile(name), mode="r:*") as layer:
                    for member in layer:
                        path = PurePosixPath(member.name)
                        if str(path) in {
                            "opt/.wh.pg-smoke",
                            "opt/.wh..wh..opq",
                            ".wh.opt",
                            ".wh..wh..opq",
                        }:
                            raise ValueError("Source tree is masked by a whiteout")
                        if not path.is_relative_to(PurePosixPath(PG)):
                            continue
                        path = name_in_tar(member.name)
                        layer_contains_tree = True
                        require(
                            path not in all_metadata
                            and member.type in TAR_MEMBERS
                            and not member.issparse(),
                            "Ambiguous source archive member",
                        )
                        member_name = "/" + str(path)
                        link = member.linkname if member.issym() else ""
                        all_metadata[path] = (
                            f"{member.mode:o} {member.uid} {member.gid} {member_name} {link}"
                        )
                        if member.isfile():
                            all_files[path] = hashlib.file_digest(
                                layer.extractfile(member), "sha256"
                            ).hexdigest()
                        if member.islnk():
                            target = name_in_tar(member.linkname)
                            require(
                                target in all_files, "Source hardlink has no bound regular target"
                            )
                            all_files[path] = all_files[target]
                        if not (source_binary(path) or source_payload(path, member)):
                            continue
                        require(str(path) not in found, "Duplicate PostgreSQL source member")
                        require(
                            member.isfile() or member.isdir() or member.issym() or member.islnk(),
                            "Invalid source member type",
                        )
                        require(
                            member.issym() or not member.mode & 0o6022,
                            "Unsafe PostgreSQL source mode",
                        )
                        require(
                            not source_binary(path) or member.isfile(),
                            "Unexpected source executable type",
                        )
                        linkname = member.linkname
                        if member.issym():
                            resolved = PurePosixPath(
                                posixpath.normpath(str(path.parent / member.linkname))
                            )
                            tree = PurePosixPath(*path.parts[:3])
                            require(
                                resolved.is_relative_to(tree),
                                "Unexpected PostgreSQL source symlink",
                            )
                            symlinks[str(path)] = str(resolved)
                        if member.islnk():
                            linkname = str(name_in_tar(member.linkname))
                            require(
                                found.get(linkname)
                                and str(path).startswith(f"{PG}/share/")
                                and linkname.startswith(f"{PG}/share/"),
                                "Unexpected PostgreSQL source hardlink",
                            )
                        budget -= 1024 + (member.size + 511) // 512 * 512
                        require(budget > 0, "Measured PostgreSQL layer budget exceeded")
                        data = (
                            layer.extractfile(member).read(member.size) if member.isfile() else b""
                        )
                        require(len(data) == member.size, "Source member size differs")
                        wanted = pg["file_sha256"].get("/" + str(path))
                        found[str(path)] = sha_bytes(data)[7:] if member.isfile() else None
                        require(
                            wanted is None or wanted == found[str(path)],
                            "Measured PostgreSQL payload differs",
                        )
                        entry = tarfile.TarInfo(str(path))
                        entry.type, entry.mode, entry.size = member.type, member.mode, member.size
                        entry.mtime, entry.linkname = member.mtime, linkname
                        destination = tools if str(path) in TOOLS_BIN else server
                        destination.addfile(entry, io.BytesIO(data) if member.isfile() else None)
                tree_count += int(layer_contains_tree)
    require(
        tree_count == 1 and (SERVER_BIN | TOOLS_BIN) <= set(found),
        "Incomplete or ambiguous source tree",
    )
    file_rows = [f"{all_files[path]}  /{path}" for path in sorted(all_files, key=str)]
    metadata_rows = sorted(all_metadata.values())
    for key, lines in (("files_sha256", file_rows), ("metadata_sha256", metadata_rows)):
        require(
            sha_bytes(("\n".join(lines) + "\n").encode())[7:] == pg[key],
            "Exact archived PostgreSQL tree differs from public baseline",
        )
    required = {
        f"{PG}/share/postgresql/postgres.bki",
        f"{PG}/lib/libpq.so.5.18",
        f"{PG}/lib/postgresql/plpgsql.so",
    }
    require(
        required <= set(found) and all(found[path] for path in required),
        "PostgreSQL installation data missing",
    )
    for name, target in symlinks.items():
        seen = {name}
        while target in symlinks:
            require(target not in seen, "PostgreSQL source symlink cycle")
            seen.add(target)
            target = symlinks[target]
        require(found.get(target), "PostgreSQL source symlink target missing")
    return found


def scratch_bytes(folder):
    total = 0
    for path in folder.rglob("*"):
        meta = path.lstat()
        require(
            meta.st_uid == os.geteuid() and not stat.S_ISLNK(meta.st_mode), "Unowned assembly input"
        )
        if stat.S_ISREG(meta.st_mode):
            total += meta.st_size
        else:
            require(stat.S_ISDIR(meta.st_mode), "Unexpected assembly input type")
    require(total <= BUDGET, "Assembly scratch exceeds 2 GiB")
    return total


def package_layers(folder, output, records, existing, *, shell_free):
    status_tar = output / ("server-status.tar" if shell_free else "tools-status.tar")
    layers = []
    with tarfile.open(status_tar, "w:") as db:
        for name, meta in sorted(records.items()):
            path = folder / "debian-inputs" / (name + ".deb")
            require(
                digest(regular(path, meta["bytes"])) == meta["sha256"],
                "Package bytes changed before assembly",
            )
            space = (meta["installed_kib"] + 8192) * 1024
            require(scratch_bytes(folder) + space <= BUDGET, "No bounded space for package payload")
            data_tar = output / (name + ".tar")
            with data_tar.open("xb") as data:
                subprocess.run(
                    ["dpkg-deb", "--fsys-tarfile", str(path)], check=True, stdout=data, timeout=120
                )
            require(data_tar.stat().st_size <= space, "Package payload exceeds its bounded space")
            with tarfile.open(data_tar, "r:") as tar:
                check_layer(tar, existing, shell_free=shell_free)
            control = subprocess.check_output(["dpkg-deb", "--field", str(path)], timeout=30)
            fields = only_record(control)
            require(
                (fields["Package"], fields["Version"], fields["Architecture"])
                == (name, meta["version"], meta["arch"])
                and "Status" not in fields
                and len(control) <= 64 * 1024,
                "Package control changed before creating status.d",
            )
            member = tarfile.TarInfo(STATUS + name)
            member.mode, member.size = 0o644, len(control)
            db.addfile(member, io.BytesIO(control))
            layers.append(data_tar)
    with tarfile.open(status_tar, "r:") as tar:
        check_layer(tar, existing, shell_free=shell_free)
    return [*layers, status_tar]


def image_command(tool, layout, source, tag, layers, *, role):
    require(re.fullmatch(r"sha256:[a-f0-9]{64}", source), "Expected an exact OCI parent digest")
    require(tag in {"common", "server", "tools"} and tag == role, "Unexpected assembly tag")
    require(
        all(re.fullmatch(r"[a-zA-Z0-9_./-]+", str(path)) for path in [layout, *layers]),
        "Unexpected OCI path",
    )
    command = [str(tool), "image", "mod", f"ocidir://{layout}@{source}", "--create", tag]
    for path in layers:
        command += ["--layer-add", f"tar={path}"]
    if role != "common":
        entry = POSTGRES if role == "server" else TOOLS_ENTRY
        args = "" if role == "server" else '["600"]'
        command += [
            "--user",
            USER,
            "--env",
            "LANG=en_US.utf8",
            "--config-entrypoint",
            json.dumps([entry]),
            "--config-cmd",
            args,
        ]
    return command


def derived_image(layout, tag, base_manifest, base_config, layers):
    desc, manifest, config = selected_image(layout, tag)
    inherited = len(base_manifest["layers"])
    diffs = config["rootfs"]["diff_ids"]
    require(
        manifest["layers"][:inherited] == base_manifest["layers"]
        and diffs[:inherited] == base_config["rootfs"]["diff_ids"]
        and diffs[inherited:] == ["sha256:" + digest(path) for path in layers],
        "OCI assembly changed inherited or appended layer bytes",
    )
    for layer, expected in zip(manifest["layers"][inherited:], diffs[inherited:], strict=True):
        filename = blob(layout, layer)
        gzip_type = layer["mediaType"] == "application/vnd.oci.image.layer.v1.tar+gzip"
        require(
            gzip_type or layer["mediaType"] == "application/vnd.oci.image.layer.v1.tar",
            "Unexpected new OCI layer media type",
        )
        with gzip.open(filename, "rb") if gzip_type else filename.open("rb") as stream:
            require(
                "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest() == expected,
                "Appended OCI blob payload differs",
            )
    if tag != "common":
        entry = POSTGRES if tag == "server" else TOOLS_ENTRY
        expected_cmd = [] if tag == "server" else ["600"]
        settings = config["config"]
        require(
            settings["User"] == USER
            and settings["Entrypoint"] == [entry]
            and (settings.get("Cmd") or []) == expected_cmd
            and "LANG=en_US.utf8" in settings["Env"]
            and not settings.get("ExposedPorts")
            and not settings.get("Volumes"),
            "OCI role runtime settings differ",
        )
    return desc, manifest, config


def assemble(folder, lock, assembly, source_receipt):
    tool = regular(folder / "regctl-linux-amd64", 64 * 1024**2)
    require(
        os.access(tool, os.X_OK) and digest(tool) == assembly["regctl"]["sha256"],
        "Reviewed regctl binary differs",
    )
    debian = lock["debian"]
    common, tools = debian["packages"], assembly["tools_packages"]
    require(
        not common.keys() & tools.keys() and not lock["base"]["packages"].keys() & tools.keys(),
        "Tool packages overlap existing vendor packages",
    )
    apt = package_inputs(
        folder / "debian-inputs", {"debian": {**debian, "packages": common | tools}}
    )
    layout = folder / "images"
    base, manifest, config, paths = vendor_inventory(
        layout, folder / "vendor-index.json", lock["base"]
    )
    with tempfile.TemporaryDirectory(dir=folder, prefix="layers-") as scratch:
        layers = Path(scratch)
        common_layers = package_layers(folder, layers, common, paths, shell_free=True)
        server_paths, tool_paths = paths.copy(), paths.copy()
        require(
            scratch_bytes(folder) + PG_BUDGET <= BUDGET,
            "No bounded space for the measured PostgreSQL layers",
        )
        source = app_layers(
            folder / "source-image.tar",
            source_receipt,
            layers / "server.tar",
            layers / "pg-tools.tar",
        )
        tool_layers = package_layers(folder, layers, tools, tool_paths, shell_free=False)
        for name, fs, shell_free in (
            ("server.tar", server_paths, True),
            ("server.tar", tool_paths, False),
            ("pg-tools.tar", tool_paths, False),
        ):
            with tarfile.open(layers / name, "r:") as tar:
                check_layer(tar, fs, shell_free=shell_free)
        for path in (
            "/usr/bin/bash",
            "/usr/bin/sleep",
            "/usr/bin/grep",
            "/usr/bin/sha256sum",
            "/usr/bin/install",
            "/usr/lib/locale/en_US.utf8/LC_COLLATE",
            "/usr/lib/locale/en_US.utf8/LC_CTYPE",
        ):
            require(
                PurePosixPath(path[1:]) in tool_paths,
                "Expected tool or exact locale file is missing",
            )
        require(
            sum(path.stat().st_size for path in (layers / "server.tar", layers / "pg-tools.tar"))
            <= PG_BUDGET,
            "Measured PostgreSQL layer budget exceeded",
        )
        results = {}
        for tag, parent, seq in (
            ("common", None, common_layers),
            ("server", "common", [layers / "server.tar"]),
            ("tools", "common", [layers / "server.tar", *tool_layers, layers / "pg-tools.tar"]),
        ):
            desc, upstream, upstream_config = (
                results[parent] if parent else (base, manifest, config)
            )
            require(
                scratch_bytes(folder) + sum(layer.stat().st_size for layer in seq) + BASE_RESERVE
                <= BUDGET,
                "No bounded space for the appended OCI blobs",
            )
            subprocess.run(
                image_command(tool, layout, desc["digest"], tag, seq, role=tag),
                env={"PATH": "/usr/bin:/bin", "LANG": "C"},
                capture_output=True,
                check=True,
                timeout=300,
            )
            results[tag] = derived_image(layout, tag, upstream, upstream_config, seq)
            scratch_bytes(folder)
        return {
            "archive_sha256": source_receipt["source_archive_sha256"],
            "signed_base_manifest": base["digest"],
            "verified_debian_packages": apt,
            "source_component_files": source,
            "images": {
                tag: {"manifest": desc["digest"], "config": meta["config"]["digest"]}
                for tag, (desc, meta, _) in results.items()
                if tag != "common"
            },
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("assembly_sha256")
    parser.add_argument("folder", type=Path)
    args = parser.parse_args()
    try:
        os.umask(0o077)
        from source import bound_source

        folder = args.folder.absolute()
        require(
            folder.parent == owned_directory(Path(os.environ["RUNNER_TEMP"]), private=False),
            "Use a fresh owner-only assembly directory inside runner scratch",
        )
        owned_directory(folder)
        owned_directory(folder / "debian-inputs")
        scratch_bytes(folder)
        assembly = scoped_source(HERE / "assembly-inputs.json", args.assembly_sha256)
        inputs = scoped_source(HERE / "inputs.json", assembly["inputs_sha256"])
        receipt = folder / "assembly-receipt.json"
        require(not os.path.lexists(receipt), "Assembly receipt path already exists")
        result = assemble(folder, inputs, assembly, bound_source(folder, assembly))
        with receipt.open("x") as report:
            report.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
    except (
        AssertionError,
        KeyError,
        ValueError,
        OSError,
        tarfile.TarError,
        subprocess.SubprocessError,
    ):
        parser.exit(1, "Private OCI assembly failed. Treat any partial layouts as unqualified.\n")


if __name__ == "__main__":
    main()
