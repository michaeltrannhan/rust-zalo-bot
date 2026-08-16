#!/usr/bin/env python3
"""Generate a CycloneDX-lite SBOM from `cargo metadata` (no extra cargo plugins)."""

from __future__ import annotations

import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import List, Optional


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    output = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "dist" / "sbom.cdx.json"
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(root / "Cargo.toml")],
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(proc.stdout)
    components = []
    for package in metadata.get("packages", []):
        components.append(
            {
                "type": "library",
                "name": package["name"],
                "version": package["version"],
                "purl": "pkg:cargo/{0}@{1}".format(package["name"], package["version"]),
                "licenses": [
                    {"license": {"id": license_id}}
                    for license_id in split_licenses(package.get("license"))
                ],
            }
        )
    version = "0.0.0"
    for pkg in metadata.get("packages", []):
        if pkg.get("name") == "zl-expense":
            version = pkg["version"]
            break
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "component": {
                "type": "application",
                "name": "zl-expense",
                "version": version,
            },
        },
        "components": components,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(output)
    return 0


def split_licenses(value: Optional[str]) -> List[str]:
    if not value:
        return []
    parts = []
    for token in value.replace("(", " ").replace(")", " ").replace("/", " ").split():
        if token.upper() in {"OR", "AND", "WITH"}:
            continue
        parts.append(token)
    return parts


if __name__ == "__main__":
    raise SystemExit(main())
