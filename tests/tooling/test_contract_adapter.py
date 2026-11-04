"""Regression controls for the restricted OpenAPI adapter."""

import copy
import json
import unittest
from pathlib import Path

from generate_contract_adapter import generate

ROOT = Path(__file__).resolve().parents[2]


class ContractAdapterTests(unittest.TestCase):
    def setUp(self):
        self.document = json.loads((ROOT / "packages/contracts/openapi.json").read_text())

    def test_current_contract_preserves_wire_metadata(self):
        result = generate(self.document)
        self.assertEqual(result, (ROOT / "packages/contracts/generated.ts").read_text())
        self.assertIn('"image/jpeg": Model.ImageBytes;', result)
        self.assertIn('"image/png": Model.ImageBytes;', result)
        self.assertIn("204: { headers: { [name: string]: unknown }; content?: never };", result)
        self.assertIn('query?: NonNullable<Model.PageData["query"]>;', result)

    def test_changed_status_and_media_change_output(self):
        changed = copy.deepcopy(self.document)
        response = changed["paths"]["/api/published/{locale}/images/{sku}/{revision}/{edge}"][
            "get"
        ]["responses"]
        response["201"] = response.pop("200")
        response["201"]["content"]["image/webp"] = response["201"]["content"].pop("image/png")
        generated = generate(changed)
        self.assertNotEqual(generated, generate(self.document))
        self.assertIn("201: { headers:", generated)
        self.assertIn('"image/webp": Model.ImageBytes;', generated)

    def test_unhandled_contract_shapes_fail_closed(self):
        changes = [
            lambda d: d["paths"]["/api/guest/basket"].update({"parameters": []}),
            lambda d: d["paths"]["/api/guest/basket"]["get"]["responses"]["200"].update(
                {"headers": {}}
            ),
            lambda d: d["paths"]["/api/guest/basket"]["get"].update(
                {
                    "parameters": [
                        {
                            "name": "session",
                            "in": "cookie",
                            "required": True,
                            "schema": {"type": "string"},
                        }
                    ]
                }
            ),
            lambda d: d["paths"]["/api/guest/basket"]["get"]["responses"]["200"]["content"][
                "application/json"
            ].update({"schema": {"type": "string"}}),
            lambda d: d["paths"]["/api/guest/basket"]["get"]["responses"].update({"default": {}}),
        ]
        for change in changes:
            with self.subTest(change=change):
                changed = copy.deepcopy(self.document)
                change(changed)
                with self.assertRaises(ValueError):
                    generate(changed)


if __name__ == "__main__":
    unittest.main()
