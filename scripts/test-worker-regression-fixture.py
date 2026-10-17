"""Real subprocess qualification of the exact staged read-only input bundle."""
import ast,hashlib,json,os,pathlib,shutil,subprocess,sys,tempfile,unittest

ROOT=pathlib.Path(__file__).resolve().parent.parent
BUNDLE=ROOT/'tests/fixtures/owned-native-recovery'
COMMIT=subprocess.check_output(['git','-C',str(ROOT),'rev-parse','HEAD'],text=True).strip()

class StagedInputs(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
  self.root=pathlib.Path(self.temp.name);self.worker=self.root/'worker';self.worker.mkdir()
  self.inputs=self.root/'readonly-input';shutil.copytree(BUNDLE,self.inputs)
  shutil.copytree(ROOT/'scripts',self.worker/'scripts',ignore=shutil.ignore_patterns('__pycache__','host-base-dispatch.py'))
  (self.worker/'data').symlink_to(self.inputs,target_is_directory=True)
  self.freeze()
 def freeze(self):
  for p in self.inputs.iterdir():
   if p.is_file():p.chmod(0o444)
 def run_file(self,name,*args):
  return subprocess.run([sys.executable,'-I',str(self.worker/'data'/name),*args],cwd=self.worker,env={**os.environ,'FACTORY_VERIFICATION_COMMIT':COMMIT},capture_output=True,text=True,timeout=20)
 def replace_checker(self,source):
  p=self.inputs/'check.py';p.chmod(0o644);p.write_text(source)
  manifest=self.inputs/'manifest.json';manifest.chmod(0o644)
  m=json.loads(manifest.read_text());m['check_sha256']=hashlib.sha256(p.read_bytes()).hexdigest();manifest.write_text(json.dumps(m));self.freeze()
 def assert_failed(self,p):
  self.assertNotEqual(p.returncode,0,p.stdout+p.stderr)
  self.assertNotIn('"passed": true',p.stdout)
 def test_exact_worker_inputs_pass_without_generated_helper(self):
  self.assertFalse((self.worker/'scripts/host-base-dispatch.py').exists())
  worker=self.run_file('check.py');self.assertEqual(worker.returncode,0,worker.stderr)
  host=self.run_file('verify.py');self.assertEqual(host.returncode,0,host.stderr)
  receipt=json.loads(host.stdout);self.assertEqual(receipt['executed'],1);self.assertEqual(receipt['commit'],COMMIT)
  self.assertFalse((self.worker/'scripts/host-base-dispatch.py').exists())
 def test_missing_dispatcher_dependency_fails_worker_and_verifier(self):
  (self.worker/'scripts/goal-runner.py').unlink()
  for name in ['check.py','verify.py']:
   p=self.run_file(name);self.assert_failed(p);self.assertIn('FileNotFoundError',p.stderr)
 def test_historical_real_audit_regression_is_detected(self):
  # Restore the actual historical bad branch. No old Git object is needed by a
  # shallow CI checkout; the check exercises dispatcher behavior, not fake output.
  target=self.worker/'scripts/research-dispatch.py';before=target.read_text()
  self.assertEqual(before.count('if already_stopped:return False'),1)
  target.write_text(before.replace('if already_stopped:return False','if already_stopped:return True'))
  for name in ['check.py','verify.py']:
   p=self.run_file(name);self.assert_failed(p);self.assertIn('Every confirmed failed attempt still requires independent controller audit',p.stderr)
 def test_nonzero_status_cannot_be_hidden_by_trailing_success_text(self):
  self.replace_checker("import sys\nprint('Ran 1 test in 0.0s\\nOK',file=sys.stderr)\nsys.exit(7)\n")
  p=self.run_file('verify.py');self.assertEqual(p.returncode,7);self.assert_failed(p)
 def test_zero_exit_with_failure_then_trailing_ok_is_rejected(self):
  self.replace_checker("import sys\nprint('Ran 1 test in 0.0s\\nFAILED (failures=1)\\nOK',file=sys.stderr)\n")
  self.assert_failed(self.run_file('verify.py'))
 def test_zero_exit_without_executed_test_is_rejected(self):
  self.replace_checker("print('unittest output was filtered away')\n")
  self.assert_failed(self.run_file('verify.py'))
 def test_missing_or_changed_frozen_check_fails_closed(self):
  p=self.inputs/'check.py';p.chmod(0o644);p.write_text("raise SystemExit(0)\n")
  self.assert_failed(self.run_file('verify.py'));p.unlink();self.assert_failed(self.run_file('verify.py'))
 def test_shell_command_overrides_are_not_accepted(self):
  self.assert_failed(self.run_file('verify.py','--command','inert output-filter request'))
 def test_original_frozen_case_body_is_unchanged(self):
  old=(self.inputs/'historical-stop-audit.py').read_bytes();origin=json.loads((self.inputs/'provenance.json').read_text())
  self.assertEqual(hashlib.sha256(old).hexdigest(),origin['historical_stop_audit_sha256'])
  cases=lambda text:next(n for n in ast.parse(text).body if isinstance(n,ast.ClassDef) and n.name=='Cases')
  self.assertEqual(ast.dump(cases(old)),ast.dump(cases((self.inputs/'check.py').read_text())))

if __name__=='__main__':unittest.main()
