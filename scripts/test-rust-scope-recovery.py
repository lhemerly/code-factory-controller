"""Frozen scope-recovery mutation assertions routed to native Rust CLI."""
import os
import importlib.util,pathlib,subprocess,sys,unittest,json
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve()
spec=importlib.util.spec_from_file_location('frozen_scope_recovery',BASE/'scripts/test-pre-admission-recovery.py');legacy=importlib.util.module_from_spec(spec);spec.loader.exec_module(legacy)
def native_advance(manifest_path,goal_id,published_path,new_base):
 p=subprocess.run([str(native),'pre-admission-recovery','advance-scope','--manifest',str(manifest_path),'--goal',goal_id,'--published-state',str(published_path),'--repair-input',new_base],capture_output=True,text=True,timeout=60)
 if p.returncode:raise ValueError(p.stderr)
 return json.loads(p.stdout)
class OrderedScopeControls(legacy.PreAdmissionRegression):
 def setUp(self):
  super().setUp();cfg=json.loads((self.root/'supervisor.json').read_text());ordered={'z_metadata':{'z':1e-7,'a':2},**cfg,'a_metadata':{'second':2,'first':1}};self.save(self.root/'supervisor.json',ordered);self.save(self.root/'goal.json',{'z_metadata':True,'id':'goal','a_metadata':{'z':1,'a':2}})
legacy_advance=legacy.A.advance_scope
names=['test_verified_baseline_advance_preserves_only_the_existing_repair_paths','test_baseline_advance_refuses_protected_changes_and_unconfirmed_ci','test_composes_only_verified_unapplied_scope_without_rewriting_policy_history','test_pending_scope_still_requires_exact_original_configuration_and_green_ci','test_pending_scope_rejects_stale_publication_and_configuration_changes']
baseline=unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(OrderedScopeControls(name) for name in names));
if not baseline.wasSuccessful():raise SystemExit('Frozen Python baseline rejected host inputs; native candidate not evaluated')
legacy.A.advance_scope=native_advance
result=unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(OrderedScopeControls(name) for name in names));raise SystemExit(not result.wasSuccessful())
