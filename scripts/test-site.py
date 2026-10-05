"""Validate website assets and public release download metadata."""
import importlib.util
from pathlib import Path
import unittest
from html.parser import HTMLParser

def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


site = load("build-site")


def release():
    result = {"tag_name": "v0.7.0", "draft": False, "prerelease": False, "assets": []}
    for key, suffix in site.TARGETS.items():
        name = f"animesh-0.7.0-{suffix}" if key.startswith("mac") else f"animesh_0.7.0_{suffix}"
        for filename in [name, name + ".sha256"]:
            result["assets"].append({"name": filename, "browser_download_url": f"https://github.com/{site.REPO}/releases/download/v0.7.0/{filename}"})
    return result


class SiteChecks(unittest.TestCase):
    def test_site_assets_and_internal_links_exist(self):
        class References(HTMLParser):
            def __init__(self):
                super().__init__()
                self.ids, self.fragments, self.assets = set(), [], []

            def handle_starttag(self, tag, attrs):
                attrs = dict(attrs)
                if "id" in attrs:
                    self.ids.add(attrs["id"])
                for field in ["src", "href"]:
                    value = attrs.get(field, "")
                    if value.startswith("assets/"):
                        self.assets.append(value)
                    elif value.startswith("#") and len(value) > 1:
                        self.fragments.append(value[1:])
        references = References()
        root = Path(__file__).resolve().parents[1] / "site"
        references.feed((root / "index.html").read_text())
        for asset in references.assets:
            self.assertTrue((root / asset).is_file(), asset)
        for fragment in references.fragments:
            self.assertIn(fragment, references.ids)

    def test_all_architecture_downloads_require_checksums(self):
        self.assertEqual(len(site.release_metadata(release())["downloads"]), 4)
        broken = release()
        broken["assets"] = [asset for asset in broken["assets"] if not asset["name"].endswith(".sha256")]
        with self.assertRaises(ValueError):
            site.release_metadata(broken)

    def test_draft_prerelease_and_untrusted_urls_are_rejected(self):
        for field in ["draft", "prerelease"]:
            broken = release()
            broken[field] = True
            with self.assertRaises(ValueError):
                site.release_metadata(broken)
        broken = release()
        broken["assets"][0]["browser_download_url"] = "https://example.com/untrusted.dmg"
        with self.assertRaises(ValueError):
            site.release_metadata(broken)



if __name__ == "__main__":
    unittest.main()
