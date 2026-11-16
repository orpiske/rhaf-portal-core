"""Require every Cargo workspace package to inherit the shared lint settings."""

import json
from pathlib import Path
import subprocess
import sys
import tomllib


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=root,
        stdout=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode:
        return result.returncode

    metadata = json.loads(result.stdout)
    members = set(metadata["workspace_members"])
    failed = False
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        manifest = Path(package["manifest_path"])
        with manifest.open("rb") as source:
            config = tomllib.load(source)
        if config.get("lints", {}).get("workspace") is not True:
            print(
                f"{manifest}: package {package['name']!r} must set [lints] workspace = true",
                file=sys.stderr,
            )
            failed = True

    if not failed:
        print(f"All {len(members)} workspace packages inherit workspace lints.")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
