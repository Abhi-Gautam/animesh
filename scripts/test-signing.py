"""Check missing credentials and Apple notarization rejection gates."""
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
    def test_public_mac_packaging_fails_before_native_commands_without_notarization(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(package, "ROOT", Path(temp)), patch.dict(os.environ, {"ANIMESH_NOTARIZED": "false"}), patch.object(package.subprocess, "run") as run:
            (Path(temp) / "Cargo.toml").write_text('[package]\nversion = "0.7.2"\n')
            with self.assertRaisesRegex(ValueError, "require Developer ID"):
                package.package("aarch64-apple-darwin")
            run.assert_not_called()

    def test_unsigned_instructions_never_recommend_gatekeeper_bypass(self):
        self.assertNotIn("Privacy &", package.MAC_INSTALL)
        self.assertIn("Never disable Gatekeeper", package.MAC_INSTALL)

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
