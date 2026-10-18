"""Run all frozen dispatcher assertions; adapt three JS caller fixtures to native audits.
No assertion is removed or changed. Remaining legacy behavior stays a parity reference.
"""
import importlib.util,pathlib,unittest
S=pathlib.Path(__file__).resolve().parent
def load(name,file):
 spec=importlib.util.spec_from_file_location(name,S/file);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
legacy=load('frozen_dispatch_all','test-research-dispatch.py');native=load('native_context_fixtures','test-rust-context-cutover.py')
original=unittest.defaultTestLoader.loadTestsFromModule(legacy)
def cases(suite):
 for item in suite:
  if isinstance(item,unittest.TestSuite):yield from cases(item)
  else:yield item
result=[]
for case in cases(original):
 name=case._testMethodName
 if isinstance(case,legacy.PermissionResolutionRegression) and name in ['test_context_admission_uses_audited_resolution_and_keeps_failure','test_host_git_context_admission_and_pending_approval_gate']:case=native.Permission(name)
 elif isinstance(case,legacy.OwnerInterruptionRegression) and name=='test_full_context_gate_uses_interruption_without_rewriting_history':case=native.Interruption(name)
 result.append(case)
assert len(result)==108, 'Frozen dispatcher regression count changed'
out=unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(result));raise SystemExit(not out.wasSuccessful())
