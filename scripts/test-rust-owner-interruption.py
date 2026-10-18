"""Frozen owner-interruption assertions routed to native Rust; no live runtime."""
import os
import importlib.util,pathlib,subprocess,sys,unittest,json
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve()
spec=importlib.util.spec_from_file_location('frozen_interruption_native',BASE/'scripts/test-owner-interruption.py');legacy=importlib.util.module_from_spec(spec);spec.loader.exec_module(legacy)
original_sha=legacy.E.sha;stat=native.stat();identity=(stat.st_dev,stat.st_ino,stat.st_size,stat.st_mtime_ns,stat.st_ctime_ns);native_sha=original_sha(native)
def fixture_sha(path):
 p=pathlib.Path(path)
 if p.resolve()==native:
  st=p.stat()
  if (st.st_dev,st.st_ino,st.st_size,st.st_mtime_ns,st.st_ctime_ns)==identity:return native_sha
 return original_sha(path)
legacy.E.sha=fixture_sha
class NativeInterruption(legacy.OwnerInterruptionRegression):
 def fixture(self):
  f=super().fixture();m=legacy.E.read(f['manifest']);m['trusted_files'][str(native)]=native_sha;f['put'](f['manifest'],m);f['seal']();return f
 def audit(self,f,n=9):
  p=subprocess.run([str(native),'owner-interruption','--root',str(f['root']),'--directory',str(f['cycle']),'--controller',str(f['controller']),'--next-sequence',str(n)],capture_output=True,text=True,timeout=45)
  if p.returncode:raise ValueError(p.stderr)
  return json.loads(p.stdout)
names=['test_real_locks_and_controller_read_preserve_failed_unknown_attempt','test_denial_unknown_error_and_running_tool_block_even_with_fresh_owner_pin','test_changed_original_history_and_owner_evidence_fail_closed','test_other_pause_is_never_cleared','test_no_pause_is_valid_only_with_same_pinned_record','test_live_recovery_identity_policy_and_stop_must_match','test_locks_sequence_and_consumption_are_required']
r=unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(NativeInterruption(n) for n in names));raise SystemExit(not r.wasSuccessful())
