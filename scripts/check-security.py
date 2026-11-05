#!/usr/bin/env python3
"""Fail on vulnerable dependencies, unexpected warnings, or unsafe release configuration."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BACKPORT_ROOT_HASH = "8ae8e4c8061697db71b692afbe61b9273eaba5eb3d9767c18934ddc3713ad68f"
GLIB_CHECKSUM = "233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5"
BACKPORT_HASH = "a0f5ee8acb8faa089bcdfbc9a57372609fce7654026ccef7d9a224d05a654ccc"

def check_config():
    patch = ROOT / "desktop/vendor/glib/src/variant_iter.rs"
    assert hashlib.sha256(patch.read_bytes()).hexdigest() == BACKPORT_HASH, "Re-review the GLib security backport"
    assert hashlib.sha256((ROOT / "desktop/vendor/glib/src/lib.rs").read_bytes()).hexdigest() == BACKPORT_ROOT_HASH
    manifest = tomllib.loads((ROOT / "desktop/Cargo.toml").read_text())
    assert manifest["patch"]["crates-io"]["glib"]["path"] == "vendor/glib"
    for workflow in (ROOT / ".github/workflows").glob("*.yml"):
        source = workflow.read_text()
        assert "persist-credentials: false" in source
        assert "permissions:\n  contents: read" in source
        for action in re.findall(r"uses: ([^\s#]+)", source):
            assert action.startswith("./") or re.fullmatch(r"[\w./-]+@[0-9a-f]{40}", action), f"Unpinned action: {action}"
    release = (ROOT / ".github/workflows/release.yml").read_text()
    assert "MACOS_SIGNING_ENABLED" not in release
    assert "environment: release" in release
    assert "attest-build-provenance@" in release
    assert "needs: [version, security]" in release
    unsigned=release.split("  build:\n",1)[1].split("  package:\n",1)[0]
    assert "secrets." not in unsigned and "environment:" not in unsigned
    signer=release.split("  package:\n",1)[1].split("  crates-io:\n",1)[0]
    assert "cargo " not in signer and "npm " not in signer
    assert "python3 scripts/sign-release.py sign" in signer


def audit(lockfile):
    contents=(ROOT / lockfile).read_text()
    local_glib='[[package]\nname = "glib"\nversion = "0.18.5"\n'
    patched_glib=lockfile=="desktop/Cargo.lock" and local_glib in contents and local_glib+'source =' not in contents
    # Local patches have no registry identity. Restore that identity only in the
    # scanner input so newly published advisories still cover the vendored crate.
    if patched_glib:
        contents=contents.replace(local_glib,local_glib+'source = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "'+GLIB_CHECKSUM+'"\n',1)
    with tempfile.TemporaryDirectory(prefix="animesh-audit-") as temporary:
        audited=Path(temporary)/"Cargo.lock";audited.write_text(contents)
        args = [os.environ.get("ANIMESH_AUDIT_BIN", "cargo-audit"), "audit", "--json", "--file", str(audited)]
        result = subprocess.run(args, cwd=ROOT, text=True, capture_output=True)
    try:
        report = json.loads(result.stdout)
    except ValueError:
        raise RuntimeError(f"Advisory scanner failed: {result.stderr}")
    assert "vulnerabilities" in report, report
    failures = report["vulnerabilities"].get("list", [])
    for category, warnings in report.get("warnings", {}).items():
        for item in warnings:
            advisory = item.get("advisory", {})
            # GLib has the exact reviewed upstream fix above, with its old ABI.
            # proc-macro-error is a build-time GTK macro dependency; it has a
            # maintenance notice, not a reported vulnerability. No runtime use.
            reviewed = lockfile.startswith("desktop/") and (
                (advisory.get("id") == "RUSTSEC-2024-0429" and item["package"]["name"] == "glib" and patched_glib and item["package"]["version"]=="0.18.5")
                or (category == "unmaintained" and advisory.get("id") == "RUSTSEC-2024-0370" and item["package"]["version"] == "1.0.4")
            )
            if reviewed:
                from datetime import date
                assert date.today() < date(2027, 2, 1), "Re-review GTK dependency exceptions by February 2027"
            else:
                failures.append(item)
    if failures:
        raise RuntimeError(json.dumps(failures, indent=2))
    assert result.returncode == 0, result.stderr
    print(f"{lockfile}: no unresolved advisories")

if __name__ == "__main__":
    try:
        check_config()
        for lockfile in ["Cargo.lock", "desktop/Cargo.lock"]:
            audit(lockfile)
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
