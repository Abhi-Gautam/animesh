"""Check missing credentials and Apple notarization rejection gates."""
import importlib.util
import json
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
