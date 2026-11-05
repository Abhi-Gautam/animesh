"""Exercise transactional user installation without touching user services or real binaries."""
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

@unittest.skipUnless(sys.platform=="linux" and os.getuid()!=0,"Linux user installer requires Linux and a non-root account")
class LinuxInstallerChecks(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name)
        self.source=self.root/"package";self.prefix=self.root/"install"
        (self.source/"bin").mkdir(parents=True);(self.source/"share").mkdir()
        shutil.copy2(ROOT/"assets/install-linux.sh",self.source/"install.sh")
        self.package("v1")

    def tearDown(self):self.temp.cleanup()

    def package(self,version,service_ok=True,desktop_ok=True):
        for name in ["animesh","animesh-app","animesh-desktop"]:
            body=f'#!/bin/sh\nif [ "$1" = "--version" ]; then echo {version}; exit {0 if name!="animesh-desktop" or desktop_ok else 1}; fi\nexit {0 if service_ok else 1}\n'
            file=self.source/"bin"/name;file.write_text(body);file.chmod(0o755)

    def install(self):
        return subprocess.run(["sh",str(self.source/"install.sh"),str(self.prefix)],text=True,capture_output=True)

    def current(self):return (self.prefix/"lib/animesh/current").resolve()

    def test_complete_upgrade_switches_all_binaries_together(self):
        self.assertEqual(self.install().returncode,0)
        old=self.current();self.package("v2")
        result=self.install();self.assertEqual(result.returncode,0,result.stderr)
        self.assertNotEqual(old,self.current());self.assertTrue(old.is_dir())
        for name in ["animesh","animesh-desktop"]:
            self.assertEqual(subprocess.check_output([str(self.prefix/"bin"/name),"--version"],text=True).strip(),"v2")

    def test_incomplete_package_never_replaces_working_release(self):
        self.assertEqual(self.install().returncode,0)
        old=self.current();(self.source/"bin/animesh-app").unlink()
        self.assertNotEqual(self.install().returncode,0);self.assertEqual(self.current(),old)

    def test_failed_start_rolls_back_to_previous_complete_release(self):
        self.assertEqual(self.install().returncode,0)
        old=self.current();self.package("v2",service_ok=False)
        self.assertNotEqual(self.install().returncode,0);self.assertEqual(self.current(),old)
        self.assertEqual(subprocess.check_output([str(self.prefix/"bin/animesh"),"--version"],text=True).strip(),"v1")

    def test_failed_binary_validation_leaves_installed_pointer_untouched(self):
        self.assertEqual(self.install().returncode,0)
        old=self.current();self.package("v2",desktop_ok=False)
        self.assertNotEqual(self.install().returncode,0);self.assertEqual(self.current(),old)

if __name__=="__main__":unittest.main()
