"""Render the AUR recipe for a tagged Astrolith release."""

import re
import sys
import tomllib
from pathlib import Path


packaging = Path(__file__).resolve().parent
root = packaging.parent.parent
with (root / "Cargo.toml").open("rb") as manifest:
    package = tomllib.load(manifest)["workspace"]["package"]

version = package["version"]
if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) or sys.argv[1:] != [f"v{version}"]:
    sys.exit(f"AUR release requires tag v{version} matching the Cargo workspace version.")

license_expression = package.get("license", "")
if not license_expression or not (root / "LICENSE").is_file():
    sys.exit("AUR release requires [workspace.package].license in Cargo.toml and a root LICENSE file.")
if not re.fullmatch(r"[A-Za-z0-9.+() -]+", license_expression):
    sys.exit("Use an SPDX license expression in [workspace.package].license.")

recipe = (packaging / "PKGBUILD.aur.in").read_text(encoding="utf-8")
output = root / "dist" / "aur" / "PKGBUILD"
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text(
    recipe.replace("@VERSION@", version).replace("@LICENSE@", license_expression),
    encoding="utf-8",
)
