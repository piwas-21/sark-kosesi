"""The actual Node filename mapper must satisfy portable artifact constraints."""

import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class EvidenceNames(unittest.TestCase):
    def test_query_variants_have_distinct_portable_diagnostic_names(self):
        result = subprocess.run(
            [
                "node",
                "--input-type=module",
                "-e",
                """import {lighthouseEvidenceName} from './tests/application/evidence-name.mjs';
const paths=['/nl','/nl/products/courtyard-demo','/nl/gallery','/nl/collection',
'/nl/collection/isolated-room?variant=SET-COVERS',
'/nl/collection/isolated-room?variant=SET-FILLED'];
console.log(JSON.stringify(paths.map(path=>lighthouseEvidenceName(path,0))));""",
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        )
        names = json.loads(result.stdout)
        self.assertEqual(len(set(names)), 6)
        self.assertEqual(names[0], "lighthouse-_nl-0.json")
        self.assertEqual(
            names[4], "lighthouse-_nl_collection_isolated-room_variant_SET-COVERS-0.json"
        )
        self.assertEqual(
            names[5], "lighthouse-_nl_collection_isolated-room_variant_SET-FILLED-0.json"
        )
        for name in names:
            self.assertRegex(name, r"^[A-Za-z0-9_.-]+$")
            self.assertNotRegex(name, r'["<>:|?*\r\n/]')
