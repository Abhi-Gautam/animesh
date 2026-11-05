"""Exercise transactional user installation without touching user services or real binaries."""
from pathlib import Path
import os
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
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

    def legacy_install(self):
        (self.prefix/"bin").mkdir(parents=True)
        for file in (self.source/"bin").iterdir():shutil.copy2(file,self.prefix/"bin"/file.name)

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

    def test_flat_v072_upgrade_preserves_previous_complete_binaries(self):
        self.legacy_install();self.package("v2")
        result=self.install();self.assertEqual(result.returncode,0,result.stderr)
        releases=list((self.prefix/"lib/animesh").glob("release.*"))
        self.assertEqual(len(releases),2)
        for name in ["animesh","animesh-app","animesh-desktop"]:
            self.assertTrue((self.prefix/"bin"/name).is_symlink())
            self.assertEqual(subprocess.check_output([str(self.prefix/"bin"/name),"--version"],text=True).strip(),"v2")
        old=next(path for path in releases if path!=self.current())
        self.assertEqual(subprocess.check_output([str(old/"bin/animesh"),"--version"],text=True).strip(),"v1")

    def test_failed_flat_upgrade_restores_all_previous_binaries(self):
        self.legacy_install();self.package("v2",service_ok=False)
        self.assertNotEqual(self.install().returncode,0)
        for name in ["animesh","animesh-app","animesh-desktop"]:
            self.assertEqual(subprocess.check_output([str(self.prefix/"bin"/name),"--version"],text=True).strip(),"v1")

    def test_incomplete_flat_installation_is_not_overwritten(self):
        self.legacy_install();(self.prefix/"bin/animesh-app").unlink();self.package("v2")
        before=(self.prefix/"bin/animesh").read_bytes()
        self.assertNotEqual(self.install().returncode,0)
        self.assertEqual((self.prefix/"bin/animesh").read_bytes(),before)
        self.assertFalse((self.prefix/"bin/animesh").is_symlink())

    def test_failed_first_install_removes_dangling_launchers(self):
        self.package("v2",service_ok=False)
        self.assertNotEqual(self.install().returncode,0)
        for name in ["animesh","animesh-app","animesh-desktop"]:
            self.assertFalse(os.path.lexists(self.prefix/"bin"/name))

    def test_concurrent_install_does_not_change_an_in_progress_upgrade(self):
        marker=self.root/"service-started"
        cli=self.source/"bin/animesh"
        cli.write_text(cli.read_text().replace("\nexit 0\n",f"\ntouch {shlex.quote(str(marker))}\nsleep 1\nexit 0\n"))
        first=subprocess.Popen(["sh",str(self.source/"install.sh"),str(self.prefix)],text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        try:
            deadline=time.monotonic()+5
            while not marker.exists():
                self.assertIsNone(first.poll(),"First install exited before the service check")
                self.assertLess(time.monotonic(),deadline);time.sleep(.01)
            second=self.install()
            self.assertNotEqual(second.returncode,0)
            self.assertIn("already running",second.stderr)
            _,stderr=first.communicate(timeout=5)
            self.assertEqual(first.returncode,0,stderr)
        finally:
            if first.poll() is None:first.kill();first.communicate()

if __name__=="__main__":unittest.main()
