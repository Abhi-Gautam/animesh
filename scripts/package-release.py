#!/usr/bin/env python3
"""Package the already verified native build; do not compile or publish it."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
MAC_INSTALL = """Move Animesh.app into /Applications and open it. Use Open Animesh
in the menu bar to open the desktop window.

To use the CLI and start the background service at login:
  mkdir -p ~/.local/bin
  ln -sf /Applications/Animesh.app/Contents/Helpers/animesh ~/.local/bin/animesh
  /Applications/Animesh.app/Contents/Helpers/animesh service start

This download is ad-hoc signed, not notarized. macOS may block it after
download. Prefer Homebrew for a source build, or use macOS's Privacy &
Security settings to allow this app. Never move the CLI out of the bundle.
"""
LINUX_INSTALL = """Built for Ubuntu 24.04 or newer. The desktop needs WebKitGTK 4.1.
The .deb release is the easiest install and resolves runtime dependencies:
  sudo apt install ./animesh_*.deb

For this tarball, first install runtime dependencies:
  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0t64 xdg-utils
Then install as your own user (do not use sudo):
  sh ./install.sh

Open Animesh from your applications menu. The background service tracks
releases when the window is closed. Your library stays in your XDG data
directory. Updates preserve it.
"""


def package(target: str) -> None:
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    output = ROOT / "target/packages"
    output.mkdir(parents=True, exist_ok=True)
    name = f"animesh-{version}-{target}"
    is_mac = target.endswith("apple-darwin")
    notarized = is_mac and os.environ.get("ANIMESH_NOTARIZED") == "true"
    if notarized:
        subprocess.run(["xcrun", "stapler", "validate", str(ROOT / "target/bundle/Animesh.app")], check=True)
    artifacts = []
    with tempfile.TemporaryDirectory(prefix="package-", dir=output) as temporary:
        work = Path(temporary)
        stage = work / name
        stage.mkdir()
        for file in ["LICENSE", "README.md"]:
            shutil.copy2(ROOT / file, stage / file)
        instructions = MAC_INSTALL if is_mac else LINUX_INSTALL
        if notarized:
            instructions = instructions.replace("This download is ad-hoc signed, not notarized. macOS may block it after\ndownload. Prefer Homebrew for a source build, or use macOS's Privacy &\nSecurity settings to allow this app. Never move the CLI out of the bundle.", "This download is Developer ID signed and notarized by Apple.\nNever move the CLI out of the bundle.")
        (stage / "INSTALL.txt").write_text(instructions)
        if is_mac:
            shutil.copytree(ROOT / "target/bundle/Animesh.app", stage / "Animesh.app", symlinks=True)
            dmg_stage = work / "dmg"
            dmg_stage.mkdir()
            shutil.copytree(stage / "Animesh.app", dmg_stage / "Animesh.app", symlinks=True)
            (dmg_stage / "Applications").symlink_to("/Applications")
            shutil.copy2(stage / "INSTALL.txt", dmg_stage / "INSTALL.txt")
            dmg = output / f"{name}.dmg"
            subprocess.run(["hdiutil", "create", "-volname", "Animesh", "-srcfolder", str(dmg_stage),
                            "-ov", "-format", "UDZO", str(dmg)], check=True)
            artifacts.append(dmg)
        else:
            bundle = ROOT / "target/bundle/animesh"
            for directory in ["bin", "share"]:
                shutil.copytree(bundle / directory, stage / directory)
            shutil.copy2(ROOT / "assets/install-linux.sh", stage / "install.sh")
            architecture = subprocess.check_output(["dpkg", "--print-architecture"], text=True).strip()
            expected = {"x86_64-unknown-linux-gnu": "amd64", "aarch64-unknown-linux-gnu": "arm64"}[target]
            if architecture != expected:
                raise RuntimeError(f"target {target} does not match build host {architecture}")
            deb = work / "deb"
            shutil.copytree(bundle, deb / "usr")
            doc = deb / "usr/share/doc/animesh"
            doc.mkdir(parents=True)
            shutil.copy2(ROOT / "LICENSE", doc / "copyright")
            # Let Debian derive ABI/version dependencies from all shipped ELF
            # binaries, rather than maintaining a hand-written library list.
            (work / "debian").mkdir()
            (work / "debian/control").write_text("Source: animesh\n\nPackage: animesh\nArchitecture: any\n")
            dependencies = subprocess.check_output(
                ["dpkg-shlibdeps", "-O", *[f"-e{file}" for file in sorted((deb / "usr/bin").iterdir())]],
                cwd=work, text=True).strip().removeprefix("shlibs:Depends=")
            control = deb / "DEBIAN"
            control.mkdir()
            (control / "control").write_text(
                f"Package: animesh\nVersion: {version}\nArchitecture: {architecture}\n"
                "Maintainer: Abhishek Gautam\nSection: utils\nPriority: optional\n"
                f"Installed-Size: {sum(file.stat().st_size for file in (deb / 'usr').rglob('*') if file.is_file()) // 1024}\n"
                f"Depends: {dependencies}, xdg-utils\n"
                "Homepage: https://github.com/Abhi-Gautam/animesh\n"
                "Description: Personal anime and TV release radar\n"
                " Local-first desktop app, background release tracking, and CLI.\n")
            archive = output / f"animesh_{version}_{architecture}.deb"
            subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(deb), str(archive)], check=True)
            artifacts.append(archive)
        archive = output / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(stage, arcname=name)
        artifacts.append(archive)
    for artifact in artifacts:
        digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
        artifact.with_name(artifact.name + ".sha256").write_text(f"{digest}  {artifact.name}\n")
        print(f"{artifact.name}: {artifact.stat().st_size / 1024 / 1024:.1f} MiB")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=["aarch64-apple-darwin", "x86_64-apple-darwin",
                                           "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"])
    package(parser.parse_args().target)
