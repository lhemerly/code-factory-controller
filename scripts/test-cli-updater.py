import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('cli',Path(__file__).with_name('code-factory.py'))
c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
class Tests(unittest.TestCase):
 def test_release_tags_are_bounded(self):
  self.assertEqual(c.release_tag('v0.2.0'),'v0.2.0')
  for tag in ('main','v0.2.0-beta','../../tmp','v1;echo bad'):
   with self.assertRaises(ValueError):c.release_tag(tag)
 def test_failed_install_keeps_active_version(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);old=root/'versions/v0.1.0';old.mkdir(parents=True);(root/'current').symlink_to(old)
   with patch.object(c,'ROOT',root),patch.object(c,'BIN',root/'bin'),patch.object(c.subprocess,'run',side_effect=subprocess.CalledProcessError(1,'git')):
    with self.assertRaises(subprocess.CalledProcessError):c.update('v0.2.0')
   self.assertEqual((root/'current').resolve(),old)
 def test_same_version_and_downgrades_do_not_build(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);current=root/'versions/v0.2.0';current.mkdir(parents=True);(root/'current').symlink_to(current)
   with patch.object(c,'ROOT',root),patch.object(c.subprocess,'run') as run:
    self.assertEqual(c.update('v0.2.0'),'v0.2.0');self.assertEqual(c.update('v0.1.0'),'v0.2.0');run.assert_not_called()
 def test_release_lookup_falls_back_to_existing_github_cli(self):
  import urllib.error
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);current=root/'versions/v0.2.0';current.mkdir(parents=True);(root/'current').symlink_to(current)
   with patch.object(c,'ROOT',root),patch.object(c.urllib.request,'urlopen',side_effect=urllib.error.HTTPError('url',404,'stale',{},None)),patch.object(c.shutil,'which',return_value='/usr/bin/gh'),patch.object(c.subprocess,'check_output',return_value=b'{"tag_name":"v0.2.0","draft":false,"prerelease":false}') as gh:
    self.assertEqual(c.update(),'v0.2.0');gh.assert_called_once()
if __name__=='__main__':unittest.main()
