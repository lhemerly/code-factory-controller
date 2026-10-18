"""Frozen JS admission gates with the native resolution commands and real owner locks.
Fake inspect transports remain test-only; no runtime denial or owner approval is changed.
"""
import importlib.util,json,os,pathlib,subprocess,unittest
S=pathlib.Path(__file__).resolve().parent
sp=importlib.util.spec_from_file_location('native_fixture',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(sp);sp.loader.exec_module(helper);fixture_temp,native=helper.native_binary(S.parent)
def load(name,file):
 sp=importlib.util.spec_from_file_location(name,S/file);module=importlib.util.module_from_spec(sp);sp.loader.exec_module(module);return module
permission=load('frozen_context_permission','test-permission-block-resolution.py');interruption=load('frozen_context_interruption','test-owner-interruption.py')
def route_fixture(f,legacy):
 # The fixture's inspect transport routes the new subcommands to the real native
 # auditor. This preserves its existing fake inspect result and checked hashes.
 controller=f['controller'];old=controller.read_text();lines=old.splitlines();injected='import os,sys\nif len(sys.argv)>1 and sys.argv[1] in ["permission-block-resolution","owner-interruption","pre-admission-recovery"]:os.execv('+repr(str(native))+',['+repr(str(native))+',*sys.argv[1:]])\n'
 controller.write_text(lines[0]+'\n'+injected+'\n'.join(lines[1:])+'\n')
 E=legacy.E if hasattr(legacy,'E') else legacy.M
 cfgpath=f['root']/'supervisor.json';cfg=E.read(cfgpath);cfg['controller_sha256']=E.sha(controller);f['put'](cfgpath,cfg)
 manifest=E.read(f['manifest']);manifest['trusted_files'][str(native)]=E.sha(native);f['put'](f['manifest'],manifest)
 if 'configuration_sha256' in E.read(f['proof']):
  proof=E.read(f['proof']);proof['configuration_sha256']=E.sha(cfgpath);f['put'](f['proof'],proof)
 f['seal']();return f
class Permission(permission.PermissionResolutionRegression):
 def fixture(self):return route_fixture(super().fixture(),permission)
class Interruption(interruption.OwnerInterruptionRegression):
 def fixture(self):return route_fixture(super().fixture(),interruption)
if __name__=='__main__':
 names=['test_context_admission_uses_audited_resolution_and_keeps_failure','test_host_git_context_admission_and_pending_approval_gate']
 suite=unittest.TestSuite([*(Permission(n) for n in names),Interruption('test_full_context_gate_uses_interruption_without_rewriting_history')]);result=unittest.TextTestRunner(verbosity=2).run(suite);raise SystemExit(not result.wasSuccessful())
