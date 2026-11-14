"""Check Mac packaging trust, missing credentials, and Apple notarization rejection gates."""
import importlib.util
import json
import io
import gzip
import tarfile
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


sign = load("sign-release")
package = load("package-release")
unpack = load("unpack-release")


class SigningChecks(unittest.TestCase):
    def package_mac(self, notarized):
        """Package a stub bundle with native tools mocked; return the tool calls and INSTALL.txt."""
        with tempfile.TemporaryDirectory() as temp, patch.object(package, "ROOT", Path(temp)), patch.dict(os.environ, {"ANIMESH_NOTARIZED": str(notarized).lower()}), patch.object(package.subprocess, "run") as run:
            root = Path(temp)
            (root / "Cargo.toml").write_text('[package]\nversion = "0.0.0"\n')
            (root / "LICENSE").write_text("license")
            (root / "README.md").write_text("readme")
            (root / "target/bundle/Animesh.app/Contents").mkdir(parents=True)
            run.side_effect = lambda args, check: Path(args[-1]).write_bytes(b"dmg") if args[0] == "hdiutil" else None
            package.package("aarch64-apple-darwin")
            with tarfile.open(root / "target/packages/animesh-0.0.0-aarch64-apple-darwin.tar.gz") as tar:
                install = tar.extractfile("animesh-0.0.0-aarch64-apple-darwin/INSTALL.txt").read().decode()
            return [call.args[0][:2] for call in run.call_args_list], install

    def test_ad_hoc_mac_package_verifies_its_seal_and_says_it_is_not_notarized(self):
        calls, install = self.package_mac(notarized=False)
        self.assertEqual(calls[0], ["codesign", "--verify"])
        self.assertNotIn(["xcrun", "stapler"], calls)
        self.assertIn("ad-hoc signed, not notarized", install)
        self.assertNotIn("notarized by Apple", install)

    def test_notarized_mac_package_validates_its_staple_and_says_so(self):
        calls, install = self.package_mac(notarized=True)
        self.assertEqual(calls[0], ["xcrun", "stapler"])
        self.assertIn("notarized by Apple", install)
        self.assertNotIn("ad-hoc", install)

    def test_mac_instructions_never_recommend_disabling_gatekeeper(self):
        for trust in package.MAC_TRUST.values():
            self.assertIn("Never disable Gatekeeper", trust)

    def test_signing_runner_only_signs_prebuilt_nested_binaries(self):
        with patch.dict(os.environ, {"ANIMESH_CODESIGN_IDENTITY": "Developer ID Application: Test (TEAM)"}), patch.object(sign, "run") as run:
            sign.sign_app(Path("unused"))
            self.assertEqual(len(run.call_args_list), 4)
            targets=[call.args[-1] for call in run.call_args_list[:3]]
            self.assertTrue(targets[0].endswith("Contents/Helpers/animesh"))
            self.assertTrue(targets[1].endswith("Animesh Desktop.app"))
            self.assertTrue(targets[2].endswith("Animesh.app"))
            self.assertTrue(all(call.args[0]=="codesign" for call in run.call_args_list))

    def test_release_extraction_rejects_paths_links_and_oversized_bodies(self):
        for name,link,size in [("../../scripts/sign-release.py",None,0),("Animesh.app/escape","../../../../outside",0),("Animesh.app/huge",None,513*1024*1024)]:
            with tempfile.TemporaryDirectory() as temp:
                archive=Path(temp)/"bundle.tar.gz"
                member=tarfile.TarInfo(name);member.size=size
                if link:member.type=tarfile.SYMTYPE;member.linkname=link
                if size:
                    # A hostile size declaration must be rejected before any body is read.
                    with gzip.open(archive,"wb") as raw:
                        raw.write(member.tobuf());raw.write(b"\0"*1024)
                else:
                    with tarfile.open(archive,"w:gz") as tar:tar.addfile(member)
                with self.assertRaises((ValueError,tarfile.FilterError)):
                    unpack.unpack("aarch64-apple-darwin",archive,Path(temp)/"output")
                self.assertFalse((Path(temp)/"outside").exists())

    def test_release_extraction_preserves_a_valid_bundle_tree(self):
        with tempfile.TemporaryDirectory() as temp:
            archive=Path(temp)/"bundle.tar.gz"
            with tarfile.open(archive,"w:gz") as tar:
                member=tarfile.TarInfo("Animesh.app/Contents/Info.plist");member.size=4
                tar.addfile(member,io.BytesIO(b"test"))
            unpack.unpack("aarch64-apple-darwin",archive,Path(temp)/"output")
            self.assertEqual((Path(temp)/"output/Animesh.app/Contents/Info.plist").read_bytes(),b"test")

    def test_missing_credentials_do_not_create_a_keychain(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {}, clear=True), patch.object(sign, "run") as run:
            with self.assertRaises(ValueError):
                sign.prepare(Path(temp))
            run.assert_not_called()

    def test_rejected_app_is_never_stapled_or_assessed(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {"APPLE_NOTARY_KEY_ID": "test", "APPLE_NOTARY_ISSUER_ID": "test"}), patch.object(sign, "run") as run:
            run.side_effect = [None, json.dumps({"status": "Invalid", "id": "test-submission"})]
            with self.assertRaises(ValueError):
                sign.notarize_app(Path(temp))
            self.assertEqual([call.args[:2] for call in run.call_args_list], [("ditto", "-c"), ("xcrun", "notarytool")])

    def test_accepted_app_is_stapled_validated_then_assessed(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {"APPLE_NOTARY_KEY_ID": "test", "APPLE_NOTARY_ISSUER_ID": "test"}), patch.object(sign, "run") as run:
            run.side_effect = [None, json.dumps({"status": "Accepted", "id": "test-submission"}), None, None, None]
            sign.notarize_app(Path(temp))
            self.assertEqual([call.args[:3] for call in run.call_args_list[2:]], [("xcrun", "stapler", "staple"), ("xcrun", "stapler", "validate"), ("spctl", "--assess", "--type")])


if __name__ == "__main__":
    unittest.main()
