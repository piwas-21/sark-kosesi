"""Source controls for the staging runtime layers; not image inspection."""

import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCKERFILE = ROOT / "deploy/staging/Dockerfile"
NODE = "node:24.22.3-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54"
DEBIAN_CC = "gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2"
DEBIAN = "gcr.io/distroless/base-nossl-debian13:nonroot@sha256:4006ac92504be607b70de01d1f64f38288c69fcd8faa820f6d3aed266ea32ac5"
LIBRARY_HELPER = ROOT / "deploy/staging/copy-runtime-libraries.sh"
LIBRARY_BUILD = [
    "COPY deploy/staging/copy-runtime-libraries.sh /copy-runtime-libraries.sh",
    "RUN --network=none --mount=type=bind,from=runtime-library-source,target=/donor sh /copy-runtime-libraries.sh /donor /runtime-libraries",
]
RUNTIME_INPUTS = [
    "COPY --from=runtime-libraries /runtime-libraries/ /",
    "COPY --from=node-runtime /etc/passwd /etc/passwd",
    "COPY --from=node-runtime /etc/group /etc/group",
]
NODE_COPIES = {
    "COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node",
    "COPY --from=node-runtime /usr/local/LICENSE /usr/local/LICENSE",
}
WEB_HEALTH_COPY = "COPY deploy/staging/web-health.mjs ./web-health.mjs"
EXPECTED_COPIES = {
    "api": {"COPY --from=rust-build /source/target/release/commerce-api /app/commerce-api"},
    "image-decoder": {
        "COPY --from=decoder-build /source/target/release/commerce-image-decoder /app/commerce-image-decoder"
    },
    "callback-edge": NODE_COPIES
    | {
        "COPY --from=rust-build /source/target/release/commerce-callback-edge /app/commerce-callback-edge"
    },
    "web": NODE_COPIES
    | {
        "COPY --from=web-build --chown=node:node /app/apps/web/.next/standalone ./",
        "COPY --from=web-build --chown=node:node /app/apps/web/.next/static ./apps/web/.next/static",
        "COPY --from=web-build --chown=node:node /app/apps/web/public ./apps/web/public",
        WEB_HEALTH_COPY,
    },
}
EXPECTED_ENTRYPOINTS = {
    "api": 'ENTRYPOINT ["/app/commerce-api"]',
    "image-decoder": 'ENTRYPOINT ["/app/commerce-image-decoder"]',
    "callback-edge": 'ENTRYPOINT ["/app/commerce-callback-edge"]',
    "web": 'ENTRYPOINT ["node", "apps/web/server.js"]',
}


def valid_runtime(source):
    stages = {}
    for line in source.splitlines():
        match = re.fullmatch(r"FROM (\S+) AS (\S+)", line)
        if match:
            parent, name = match.groups()
            if name in stages:
                return False
            stages[name] = [parent, []]
        elif line.strip() and not line.lstrip().startswith("#"):
            if not stages:
                return False
            stages[next(reversed(stages))][1].append(line)
    if stages.get("decoder-build") != [
        "rust-source",
        ["RUN cargo build --release --locked -p commerce-image-decoder"],
    ]:
        return False
    if stages.get("node-runtime") != [NODE, []]:
        return False
    if stages.get("runtime-library-source") != [DEBIAN_CC, []]:
        return False
    if stages.get("runtime-libraries") != ["node-runtime", LIBRARY_BUILD]:
        return False
    if stages.get("runtime") != [DEBIAN, RUNTIME_INPUTS]:
        return False
    if stages.get("web-build", [None])[0] != "node-runtime":
        return False
    for name, expected in EXPECTED_COPIES.items():
        parent, lines = stages.get(name, [None, []])
        copies = [line for line in lines if line.split(maxsplit=1)[0].upper() == "COPY"]
        users = [line for line in lines if line.split(maxsplit=1)[0].upper() == "USER"]
        entrypoints = [line for line in lines if line.split(maxsplit=1)[0].upper() == "ENTRYPOINT"]
        allowed = {"ARG", "LABEL", "WORKDIR", "COPY", "USER", "ENTRYPOINT", "ENV"}
        if (
            parent != "runtime"
            or len(copies) != len(expected)
            or set(copies) != expected
            or any(line.split(maxsplit=1)[0].upper() not in allowed for line in lines)
            or users != ["USER 65532:1000" if name == "image-decoder" else "USER node"]
            or entrypoints != [EXPECTED_ENTRYPOINTS[name]]
        ):
            return False
    return True


class StagingRuntime(unittest.TestCase):
    def test_final_stage_source_has_bounded_runtime_inputs(self):
        self.assertTrue(valid_runtime(DOCKERFILE.read_text()))

    def test_detector_rejects_runtime_regressions(self):
        source = DOCKERFILE.read_text()
        for changed in (
            source.replace(f"FROM {DEBIAN} AS runtime", f"FROM {NODE} AS runtime"),
            source.replace("FROM runtime AS api", "FROM node-runtime AS api"),
            source.replace(
                "COPY --from=node-runtime /usr/local/LICENSE /usr/local/LICENSE\n", "", 1
            ),
            source.replace(
                "FROM runtime AS web\n",
                "FROM runtime AS web\nCOPY --from=node-runtime /usr/local/lib/node_modules /usr/local/lib/node_modules\n",
            ),
            source.replace(
                "FROM runtime AS callback-edge\n",
                "FROM runtime AS callback-edge\nRUN npm install\n",
            ),
            source.replace("USER 65532:1000", "USER 1000:1000"),
            source.replace(
                "FROM rust-source AS decoder-build", "FROM node-runtime AS decoder-build"
            ),
            source.replace("FROM runtime AS image-decoder", "FROM node-runtime AS image-decoder"),
            source.replace(
                "FROM runtime AS callback-edge\n",
                "FROM runtime AS callback-edge\ncopy --from=node-runtime /usr/local/lib /usr/local/lib\n",
            ),
            source.replace(
                'ENTRYPOINT ["/app/commerce-api"]', 'USER root\nENTRYPOINT ["/app/commerce-api"]'
            ),
            source.replace(RUNTIME_INPUTS[0] + "\n", ""),
            source.replace(RUNTIME_INPUTS[1] + "\n", ""),
            source.replace(WEB_HEALTH_COPY + "\n", ""),
            source.replace(WEB_HEALTH_COPY, "COPY deploy/staging/ ./"),
            source.replace(RUNTIME_INPUTS[2] + "\n", ""),
            source.replace(DEBIAN + " AS runtime", DEBIAN_CC + " AS runtime"),
            source.replace(DEBIAN_CC, "gcr.io/distroless/cc-debian13:nonroot"),
            source.replace("--network=none", "--network=default"),
            source.replace("from=runtime-library-source,target", "from=node-runtime,target"),
        ):
            self.assertNotEqual(changed, source)
            self.assertFalse(valid_runtime(changed))


class RuntimeLibraryOverlay(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.donor = Path(temporary.name) / "donor"
        self.output = Path(temporary.name) / "output"
        self.expected = set()
        self.cc_library = "usr/lib/aarch64-linux-gnu/libstdc++.so.6"
        gomp_library = "usr/lib/aarch64-linux-gnu/libgomp.so.1"
        packages = {
            "gcc-14-base": "usr/share/doc/gcc-14-base/copyright",
            "libgcc-s1": "usr/lib/aarch64-linux-gnu/libgcc_s.so.1",
            "libstdc++6": self.cc_library + ".0.33",
            "libgomp1": gomp_library + ".0.0",
        }
        for package, payload in packages.items():
            self.write(payload, "synthetic package file\n")
            status = f"var/lib/dpkg/status.d/{package}"
            self.write(status, f"Package: {package}\n")
            self.write(status + ".md5sums", f"800b2be505e9eaa00116f6917ec97b2f  {payload}\n")
            if package != "gcc-14-base":
                self.link(f"usr/share/doc/{package}", "gcc-14-base")
        self.link(self.cc_library, "libstdc++.so.6.0.33")
        self.link(gomp_library, "libgomp.so.1.0.0")
        self.write("usr/lib/aarch64-linux-gnu/libssl.so.3", "unrelated library", selected=False)
        self.write("var/lib/dpkg/status.d/libssl3t64", "unrelated status", selected=False)

    def write(self, name, content, *, selected=True):
        entry = self.donor / name
        entry.parent.mkdir(parents=True, exist_ok=True)
        entry.write_text(content)
        if selected:
            self.expected.add(name)

    def link(self, name, target):
        entry = self.donor / name
        entry.parent.mkdir(parents=True, exist_ok=True)
        entry.symlink_to(target)
        self.expected.add(name)

    def copy(self):
        return subprocess.run(
            ["sh", str(LIBRARY_HELPER), str(self.donor), str(self.output)],
            capture_output=True,
            timeout=10,
        )

    def test_copies_complete_packages_and_soname_links_only(self):
        result = self.copy()
        self.assertEqual(result.returncode, 0, result.stderr)
        actual = {
            entry.relative_to(self.output).as_posix()
            for entry in self.output.rglob("*")
            if entry.is_file() or entry.is_symlink()
        }
        self.assertEqual(actual, self.expected)
        for name in self.expected:
            source, target = self.donor / name, self.output / name
            if source.is_symlink():
                self.assertEqual(source.readlink(), target.readlink())
            else:
                self.assertEqual(source.read_bytes(), target.read_bytes())

    def test_rejects_integrity_mismatch(self):
        (self.donor / (self.cc_library + ".0.33")).write_text("corrupted package file")
        self.assertNotEqual(self.copy().returncode, 0)

    def test_rejects_missing_soname(self):
        (self.donor / self.cc_library).unlink()
        self.assertNotEqual(self.copy().returncode, 0)

    def test_rejects_missing_metadata(self):
        (self.donor / "var/lib/dpkg/status.d/libgcc-s1").unlink()
        self.assertNotEqual(self.copy().returncode, 0)
