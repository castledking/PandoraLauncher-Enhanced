#!/usr/bin/env python3
"""Re-apply local gpui-component fixes after `cargo vendor`.

`cargo vendor` rewrites vendor/ straight from the upstream git sources, so
anything we change there is gone the next time vendor.sh runs. The fixes are
kept as a patch next to the vendored crate and put back here, along with the
`.cargo-checksum.json` entries cargo checks the directory source against.
"""

import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRATE = ROOT / "vendor" / "gpui-component"
PATCHES = sorted((ROOT / "patches" / "gpui-component").glob("*.patch"))

if not CRATE.is_dir():
    sys.exit(f"{CRATE} is missing - run `cargo vendor` first")
if not PATCHES:
    sys.exit("no patches to apply")


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def patch_targets(text: str) -> list[str]:
    targets = []
    for line in text.splitlines():
        if line.startswith("+++ b/"):
            targets.append(line[len("+++ b/") :].strip())
    return targets


def run_patch(patch: pathlib.Path, *args: str) -> bool:
    result = subprocess.run(
        ["patch", "--quiet", "-p1", *args, "-i", str(patch)],
        cwd=CRATE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return result.returncode == 0


targets = []
for patch in PATCHES:
    patch_files = patch_targets(patch.read_text())
    if not patch_files:
        sys.exit(f"{patch} lists no files")

    # Already applied is fine; checked first so patch doesn't leave rejects behind
    already = run_patch(patch, "--reverse", "--dry-run")
    if not already and not run_patch(patch, "--forward"):
        # Anything else means the upstream file moved.
        sys.exit(f"failed to apply {patch}")

    targets.extend(t for t in patch_files if t not in targets)

checksums = CRATE / ".cargo-checksum.json"
data = json.loads(checksums.read_text())
changed = False
for target in targets:
    path = CRATE / target
    if not path.is_file():
        sys.exit(f"patched file {path} is missing")
    digest = sha256(path)
    if data["files"].get(target) != digest:
        data["files"][target] = digest
        changed = True

if changed:
    checksums.write_text(json.dumps(data, separators=(",", ":")))

print(f"patched gpui-component: {', '.join(targets)}")
