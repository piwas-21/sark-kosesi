"""Static isolation policy for the optional decoder overlay; not runtime proof."""

import copy
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OVERLAY = ROOT / "deploy/staging/image-decoder.compose.yml"
SOCKET = "/decoder-socket/decoder.sock"
SOURCE = "${STAGING_IMAGE_DECODER_DIR:?pre-provisioned private decoder directory required}"
IMAGE = "sark-kosesi-image-decoder:${STAGING_REVISION:?reviewed source revision required}"


def valid_overlay(document):
    if not isinstance(document, dict) or set(document) != {"services"}:
        return False
    services = document["services"]
    if not isinstance(services, dict) or set(services) != {"api", "image-decoder"}:
        return False
    api = services["api"]
    decoder = services["image-decoder"]
    if not isinstance(api, dict) or not isinstance(decoder, dict):
        return False
    mount = {
        "type": "bind",
        "source": SOURCE,
        "target": "/decoder-socket",
        "bind": {"create_host_path": False},
    }
    if api != {
        "environment": {"PREVIEW_IMAGE_DECODER_SOCKET": SOCKET},
        "volumes": [mount | {"read_only": True}],
    }:
        return False
    return decoder == {
        "profiles": ["image-decoder"],
        "image": IMAGE,
        "user": "65532:1000",
        "command": ["serve", SOCKET, "1000"],
        "network_mode": "none",
        "read_only": True,
        "volumes": [mount],
        "restart": "no",
        "mem_limit": "1g",
        "memswap_limit": "1g",
        "cpus": 1.0,
        "pids_limit": 32,
        "cap_drop": ["ALL"],
        "security_opt": ["no-new-privileges:true"],
    }


class ImageDecoderOverlay(unittest.TestCase):
    def test_optional_overlay_has_isolated_resource_bounded_runtime(self):
        self.assertTrue(valid_overlay(json.loads(OVERLAY.read_text())))

    def test_detector_rejects_relaxed_isolation_and_missing_limits(self):
        source = json.loads(OVERLAY.read_text())
        mutations = [
            ("network_mode", "host"),
            ("user", "1000:1000"),
            ("read_only", False),
            ("mem_limit", "2g"),
            ("memswap_limit", "2g"),
            ("cpus", 2.0),
            ("pids_limit", 64),
            ("cap_drop", []),
            ("security_opt", []),
            ("profiles", []),
            ("restart", "always"),
            ("environment", {"DATABASE_URL": "secret"}),
            ("ports", ["45673:45673"]),
        ]
        for key, value in mutations:
            changed = copy.deepcopy(source)
            changed["services"]["image-decoder"][key] = value
            with self.subTest(key=key):
                self.assertFalse(valid_overlay(changed))
        for target in ("api", "image-decoder"):
            changed = copy.deepcopy(source)
            changed["services"][target]["volumes"][0]["bind"]["create_host_path"] = True
            self.assertFalse(valid_overlay(changed))
        changed = copy.deepcopy(source)
        changed["services"]["api"]["volumes"][0]["read_only"] = False
        self.assertFalse(valid_overlay(changed))
        changed = copy.deepcopy(source)
        changed["services"]["image-decoder"]["volumes"].append({"type": "bind", "source": "/"})
        self.assertFalse(valid_overlay(changed))

    def test_base_compose_does_not_enable_decoder(self):
        text = (ROOT / "deploy/staging/compose.yml").read_text()
        self.assertNotIn("PREVIEW_IMAGE_DECODER_SOCKET", text)
        self.assertNotIn("image-decoder", text)
