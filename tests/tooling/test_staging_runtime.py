"""Source controls for the staging runtime layers; not image inspection."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCKERFILE = ROOT / "deploy/staging/Dockerfile"
NODE = "node:24.22.3-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54"
DEBIAN = (
    "debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc"
)
IDENTITY = "RUN groupadd --gid 1000 node && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash node"
NODE_COPIES = {
    "COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node",
    "COPY --from=node-runtime /usr/local/LICENSE /usr/local/LICENSE",
}
EXPECTED_COPIES = {
    "api": {"COPY --from=rust-build /source/target/release/commerce-api /app/commerce-api"},
    "callback-edge": NODE_COPIES
    | {
        "COPY --from=rust-build /source/target/release/commerce-callback-edge /app/commerce-callback-edge"
    },
    "web": NODE_COPIES
    | {
        "COPY --from=web-build --chown=node:node /app/apps/web/.next/standalone ./",
        "COPY --from=web-build --chown=node:node /app/apps/web/.next/static ./apps/web/.next/static",
        "COPY --from=web-build --chown=node:node /app/apps/web/public ./apps/web/public",
    },
}
EXPECTED_ENTRYPOINTS = {
    "api": 'ENTRYPOINT ["/app/commerce-api"]',
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
    if stages.get("node-runtime") != [NODE, []]:
        return False
    if stages.get("runtime") != [DEBIAN, [IDENTITY]]:
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
            or users != ["USER node"]
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
            source.replace(
                "FROM runtime AS callback-edge\n",
                "FROM runtime AS callback-edge\ncopy --from=node-runtime /usr/local/lib /usr/local/lib\n",
            ),
            source.replace(
                'ENTRYPOINT ["/app/commerce-api"]', 'USER root\nENTRYPOINT ["/app/commerce-api"]'
            ),
            source.replace(IDENTITY, "RUN useradd node"),
        ):
            self.assertNotEqual(changed, source)
            self.assertFalse(valid_runtime(changed))
