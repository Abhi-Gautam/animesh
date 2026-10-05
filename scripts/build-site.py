#!/usr/bin/env python3
"""Build the static launch site using a verified public release's download URLs."""
import argparse
import json
from pathlib import Path
import re
import shutil
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
REPO = "Abhi-Gautam/animesh"
TARGETS = {"mac-arm": "aarch64-apple-darwin.dmg", "mac-intel": "x86_64-apple-darwin.dmg",
           "linux-amd64": "amd64.deb", "linux-arm64": "arm64.deb"}


def release_metadata(release):
    version = release["tag_name"]
    if not re.fullmatch(r"v\d+\.\d+\.\d+", version) or release.get("draft") or release.get("prerelease"):
        raise ValueError("Expected a stable, public release")
    assets = {asset["name"]: asset["browser_download_url"] for asset in release["assets"]}
    downloads = {}
    for key, suffix in TARGETS.items():
        name = f"animesh-{version[1:]}-{suffix}" if key.startswith("mac") else f"animesh_{version[1:]}_{suffix}"
        url = assets[name]
        expected = f"https://github.com/{REPO}/releases/download/{version}/{name}"
        if url != expected or name + ".sha256" not in assets:
            raise ValueError(f"Missing checksum or unexpected download URL for {name}")
        downloads[key] = url
    return {"version": version, "downloads": downloads}


def build(release):
    metadata = release_metadata(release)
    source = ROOT / "site"
    for file in [source / "index.html", source / "styles.css", source / "app.js"]:
        for asset in re.findall(r"assets/[\w./-]+", file.read_text()):
            # JS constructs screenshot URLs; concrete filenames are checked below.
            if asset.endswith("/"):
                continue
            if not (source / asset).is_file():
                raise ValueError(f"Missing site asset: {asset}")
    for name in re.findall(r'"([\w-]+\.png)"', (source / "app.js").read_text()):
        if not (source / "assets/screenshots" / name).is_file():
            raise ValueError(f"Missing screenshot: {name}")
    output = ROOT / "target/site"
    if output.exists():
        shutil.rmtree(output)
    shutil.copytree(source, output, ignore=shutil.ignore_patterns("README.md", "launch.html", "terminal-*.html"))
    (output / "release.json").write_text(json.dumps(metadata, indent=2) + "\n")
    html = (output / "index.html").read_text()
    for key, url in metadata["downloads"].items():
        html = re.sub(fr'(data-download="{key}" href=")[^"]+', lambda match: match[1] + url, html)
    html = re.sub(r'(<span id="release-version">)[^<]+', lambda match: match[1] + metadata["version"], html)
    (output / "index.html").write_text(html)
    print(f"Built {output} for {metadata['version']}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-json", type=Path)
    args = parser.parse_args()
    if args.release_json:
        release = json.loads(args.release_json.read_text())
    else:
        request = urllib.request.Request(f"https://api.github.com/repos/{REPO}/releases/latest", headers={"User-Agent": "animesh-site-build", "Accept": "application/vnd.github+json"})
        with urllib.request.urlopen(request, timeout=30) as response:
            release = json.load(response)
    build(release)
