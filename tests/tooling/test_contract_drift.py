"""Exercise the actual generator and detect tampering at each drift boundary."""

import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import check_contracts

ROOT = Path(__file__).resolve().parents[2]


class ContractDriftTests(unittest.TestCase):
    def test_generation_passes_and_three_drift_boundaries_fail(self):
        with tempfile.TemporaryDirectory(dir=ROOT / ".local") as temporary:
            root = Path(temporary)
            target = root / "packages/contracts"
            target.mkdir(parents=True)
            for name in ("openapi.json", "generated.ts", "models.gen.ts"):
                shutil.copyfile(ROOT / "packages/contracts" / name, target / name)
            (root / "node_modules").symlink_to(ROOT / "node_modules", target_is_directory=True)
            openapi = (target / "openapi.json").read_bytes()
            with (
                patch.object(check_contracts, "ROOT", root),
                patch.object(
                    check_contracts.subprocess, "check_output", return_value=openapi
                ) as cargo,
            ):
                check_contracts.main()
                for name in ("generated.ts", "models.gen.ts"):
                    with self.subTest(name=name):
                        path = target / name
                        original = path.read_bytes()
                        path.write_bytes(original + b"\n")
                        with self.assertRaisesRegex(ValueError, "drift"):
                            check_contracts.main()
                        path.write_bytes(original)
                cargo.return_value = openapi + b"\n"
                with self.assertRaisesRegex(ValueError, "Rust OpenAPI drift"):
                    check_contracts.main()


if __name__ == "__main__":
    unittest.main()
