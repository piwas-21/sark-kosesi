#!/usr/bin/env python3
"""Fail until the current UI and every screenshot have explicit human approval."""

import json
from pathlib import Path

from visual_policy import validate

ROOT = Path(__file__).resolve().parents[1]
if __name__ == "__main__":
    try:
        validate(json.loads((ROOT / "config/visual-approval.json").read_text()), ROOT)
    except (OSError, ValueError, TypeError) as error:
        raise SystemExit(f"Visual baseline approval missing/stale: {error}") from None
    print("Current UI source and all visual baseline hashes have manual approval")
