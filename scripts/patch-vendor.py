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
PATCH = ROOT / "patches" / "gpui-component" / "line-height.patch"

if not CRATE.is_dir():
    sys.exit(f"{CRATE} is missing - run `cargo vendor` first")


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def patch_targets(text: str) -> list[str]:
    targets = []
    for line in text.splitlines():
        if line.startswith("+++ b/"):
            targets.append(line[len("+++ b/") :].strip())
    return targets


targets = patch_targets(PATCH.read_text())
if not targets:
    sys.exit(f"{PATCH} lists no files")

result = subprocess.run(
    ["patch", "--quiet", "--forward", "-p1", "-i", str(PATCH)],
    cwd=CRATE,
)
if result.returncode != 0:
    # Already applied is fine; anything else means the upstream file moved.
    already = all(
        "1.25" in (CRATE / target).read_text()
        for target in targets
        if (CRATE / target).is_file()
    )
    if not already:
        sys.exit(f"failed to apply {PATCH}")

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