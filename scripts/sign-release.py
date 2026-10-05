#!/usr/bin/env python3
"""Prepare ephemeral Apple signing credentials, notarize, and clean up on CI."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def run(*args, capture=False):
    # Never print commands: security arguments contain certificate passwords.
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE if capture else None).stdout


def prepare(directory):
    required = ["APPLE_CERTIFICATE_P12", "APPLE_CERTIFICATE_PASSWORD", "ANIMESH_CODESIGN_IDENTITY",
                "APPLE_NOTARY_KEY_P8", "APPLE_NOTARY_KEY_ID", "APPLE_NOTARY_ISSUER_ID"]
    missing = [name for name in required if not os.environ.get(name)]
    if missing:
        raise ValueError("Signing enabled but missing: " + ", ".join(missing))
    if not os.environ["ANIMESH_CODESIGN_IDENTITY"].startswith("Developer ID Application:"):
        raise ValueError("Public distribution requires a Developer ID Application identity")
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    directory.chmod(0o700)
    certificate = directory / "certificate.p12"
    certificate.write_bytes(base64.b64decode(os.environ["APPLE_CERTIFICATE_P12"], validate=True))
    certificate.chmod(0o600)
    key = directory / "notary.p8"
    key.write_text(os.environ["APPLE_NOTARY_KEY_P8"])
    key.chmod(0o600)
    keychain = directory / "signing.keychain-db"
    original = shlex.split(run("security", "list-keychains", "-d", "user", capture=True))
    (directory / "state.json").write_text(json.dumps({"keychains": original}))
    password = secrets.token_urlsafe(32)
    run("security", "create-keychain", "-p", password, str(keychain))
    run("security", "set-keychain-settings", "-lut", "21600", str(keychain))
    run("security", "unlock-keychain", "-p", password, str(keychain))
    run("security", "import", str(certificate), "-P", os.environ["APPLE_CERTIFICATE_PASSWORD"], "-k", str(keychain), "-T", "/usr/bin/codesign")
    run("security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, str(keychain))
    run("security", "list-keychains", "-d", "user", "-s", str(keychain), *original)
    identities = run("security", "find-identity", "-v", "-p", "codesigning", str(keychain), capture=True)
    if os.environ["ANIMESH_CODESIGN_IDENTITY"] not in identities:
        raise ValueError("The imported certificate does not contain the configured signing identity")
    certificate.unlink()
    print("Developer ID signing credentials prepared in an ephemeral keychain")


def notarize(path, directory):
    result = json.loads(run("xcrun", "notarytool", "submit", str(path), "--key", str(directory / "notary.p8"),
                            "--key-id", os.environ["APPLE_NOTARY_KEY_ID"], "--issuer", os.environ["APPLE_NOTARY_ISSUER_ID"],
                            "--wait", "--timeout", "30m", "--output-format", "json", capture=True))
    (directory / (path.name + ".notary.json")).write_text(json.dumps(result, indent=2))
    if result.get("status") != "Accepted":
        raise ValueError(f"Apple rejected {path.name}; submission {result.get('id')}. Retrieve its notarytool log for diagnostics.")
    print(f"Apple accepted {path.name}: {result['id']}")


def notarize_app(directory):
    app = ROOT / "target/bundle/Animesh.app"
    archive = directory / "Animesh.zip"
    run("ditto", "-c", "-k", "--keepParent", str(app), str(archive))
    notarize(archive, directory)
    run("xcrun", "stapler", "staple", str(app))
    run("xcrun", "stapler", "validate", str(app))
    run("spctl", "--assess", "--type", "execute", "--verbose", str(app))


def notarize_packages(directory):
    packages = ROOT / "target/packages"
    images = list(packages.glob("*.dmg"))
    if len(images) != 1:
        raise ValueError("Expected exactly one native disk image")
    image = images[0]
    run("codesign", "--force", "--sign", os.environ["ANIMESH_CODESIGN_IDENTITY"], "--timestamp", str(image))
    notarize(image, directory)
    run("xcrun", "stapler", "staple", str(image))
    run("xcrun", "stapler", "validate", str(image))
    run("spctl", "--assess", "--type", "open", "--context", "context:primary-signature", "--verbose", str(image))
    # Stapling changes the disk image; publish checksums only after that change.
    for file in [image, *packages.glob("*.tar.gz")]:
        file.with_name(file.name + ".sha256").write_text(f"{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.name}\n")


def cleanup(directory):
    state = directory / "state.json"
    keychain = directory / "signing.keychain-db"
    try:
        if state.exists():
            run("security", "list-keychains", "-d", "user", "-s", *json.loads(state.read_text())["keychains"])
    finally:
        try:
            if keychain.exists():
                run("security", "delete-keychain", str(keychain))
        finally:
            for file in directory.glob("*"):
                if file.is_file():
                    file.unlink()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "app", "packages", "cleanup"])
    args = parser.parse_args()
    directory = Path(os.environ["RUNNER_TEMP"]) / "animesh-signing"
    try:
        {"prepare": prepare, "app": notarize_app, "packages": notarize_packages, "cleanup": cleanup}[args.action](directory)
    except Exception as error:
        # CalledProcessError includes arguments (and passwords). Report only the type.
        if isinstance(error, subprocess.CalledProcessError):
            print(f"Apple signing command failed with exit {error.returncode}", file=sys.stderr)
        else:
            print(str(error), file=sys.stderr)
        sys.exit(1)
