"""Independent host regression adapter: original frozen fixtures, native Rust CLI."""
import os
import importlib.util,pathlib,subprocess,sys,unittest,json,hashlib
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve()
spec=importlib.util.spec_from_file_location('frozen_resolution',BASE/'scripts/test-permission-block-resolution.py');legacy=importlib.util.module_from_spec(spec);spec.loader.exec_module(legacy)
# The frozen producer repeatedly rehashes every already-pinned fixture input.
# Cache only the large immutable compiled artifact; the Rust auditor still hashes
# its running executable independently on every audit. Any metadata change forces rehash.
_original_sha=legacy.M.sha
_initial_stat=native.stat()
_native_identity=(_initial_stat.st_dev,_initial_stat.st_ino,_initial_stat.st_size,_initial_stat.st_mtime_ns,_initial_stat.st_ctime_ns)
_native_sha=_original_sha(native)
def fixture_sha(path):
 p=pathlib.Path(path)
 if p.resolve()==native:
  st=p.stat();identity=(st.st_dev,st.st_ino,st.st_size,st.st_mtime_ns,st.st_ctime_ns)
  if identity==_native_identity:return _native_sha
 return _original_sha(path)
legacy.M.sha=fixture_sha

class RustPermissionControls(legacy.PermissionResolutionRegression):
 def fixture(self):
  f=super().fixture();m=legacy.M.read(f['manifest']);m['trusted_files'][str(native)]=legacy.M.sha(native);f['put'](f['manifest'],m);f['seal']();return f
 def audit(self,f,next_sequence=4):
  out=subprocess.run([str(native),'permission-block-resolution','--root',str(f['root']),'--directory',str(f['cycle']),'--controller',str(f['controller']),'--next-sequence',str(next_sequence)],capture_output=True,text=True,timeout=45)
  if out.returncode:raise ValueError(out.stderr)
  return json.loads(out.stdout)
 def test_native_implementation_pin_cannot_be_omitted(self):
  f=self.fixture();m=legacy.M.read(f['manifest']);del m['trusted_files'][str(native)];f['put'](f['manifest'],m);f['seal']()
  with self.assertRaisesRegex(ValueError,'[Ii]mplementation|[Ee]xecutable|[Rr]esolver|[Pp]in'):self.audit(f)
 def test_native_implementation_pin_cannot_be_wrong(self):
  f=self.fixture();m=legacy.M.read(f['manifest']);m['trusted_files'][str(native)]='0'*64
  # Seal must retain the deliberately bad implementation pin, instead of rehashing it.
  proof=legacy.M.read(f['proof']);m['trusted_files'].pop(str(f['proof']));proof['manifest_without_resolution_sha256']=legacy.M.digest(m);f['put'](f['proof'],proof);m['trusted_files'][str(f['proof'])]=legacy.M.sha(f['proof']);f['put'](f['manifest'],m);f['put'](f['owner_state'],{'manifest_digest':legacy.M.digest(m)})
  with self.assertRaises(ValueError):self.audit(f)
# JS-context/resume tests below call legacy Python directly and therefore cannot
# establish Rust parity. Select only original tests that route through self.audit.
selected=['test_host_git_resolution_preserves_failure_and_never_approves_code','test_host_git_rejects_other_denials_even_when_owner_repins_history','test_host_git_requires_new_approval_contract_and_verified_runtime','test_isolated_owner_checkout_resolution_preserves_subsequent_actions','test_isolation_resolution_rejects_missing_or_changed_qualification','test_preserves_original_unknown_usage_and_denial','test_next_preflight_directory_allowed_but_admission_consumes_resolution','test_stop_and_accounting_controls','test_owner_authorization_and_evidence_controls','test_no_unpinned_or_symlink_resolution_and_no_user_pause','test_exact_denial_cannot_resolve_another_action','test_authorization_cannot_waive_unknown_usage_or_expose_file','test_packaging_summary_cannot_hide_weakened_acceptance','test_native_implementation_pin_cannot_be_omitted','test_native_implementation_pin_cannot_be_wrong']
suite=unittest.TestSuite(RustPermissionControls(name) for name in selected);result=unittest.TextTestRunner(verbosity=2).run(suite);raise SystemExit(not result.wasSuccessful())
