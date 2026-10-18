"""Native review audit of frozen real-controller delivery fixture; no remote writes."""
import importlib.util,json,os,pathlib,subprocess,unittest
S=pathlib.Path(__file__).resolve().parent
sp=importlib.util.spec_from_file_location('native_fixture',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(sp);sp.loader.exec_module(helper);fixture_temp,binary=helper.native_binary(S.parent);os.environ['FACTORY_FIXTURE_CONTROLLER']=str(binary)
sp=importlib.util.spec_from_file_location('frozen_github_controller',S/'test-github-controller.py');legacy=importlib.util.module_from_spec(sp);sp.loader.exec_module(legacy)
class NativeAudit(legacy.ControllerBoundary):
 def invoke(self):
  return subprocess.run([str(binary),'github-review','audit','--state',str(self.adapter.root),'--batch',self.key],capture_output=True,text=True,timeout=45)
 def test_native_audit_binds_authoritative_receipt_and_rejects_tampered_archive(self):
  p=self.invoke();self.assertEqual(p.returncode,0,p.stderr);out=json.loads(p.stdout);record=legacy.M.read(self.result);self.assertEqual(out['commit'],record['acceptance']['integrated_commit']);self.assertIn('Verified checks:',out['body'])
  record['acceptance']['active_verification_receipt']['environment']['checks'][0]['observations']={'executed':99999};legacy.M.save(self.result,record)
  p=self.invoke();self.assertEqual(p.returncode,0,p.stderr);self.assertNotIn('99999',json.loads(p.stdout)['body'])
  archive=pathlib.Path(record['acceptance']['delivery']['destination']);archive.write_bytes(archive.read_bytes()+b'tampered');self.assertNotEqual(self.invoke().returncode,0)
 def test_native_audit_refuses_forged_receipt_and_changed_host_policy(self):
  out=legacy.M.read(self.result);out['acceptance']['active_verification_receipt']['verification_id']='forged';legacy.M.save(self.result,out);self.assertNotEqual(self.invoke().returncode,0)
  (self.goal/'supervisor.json').write_text('{}');self.assertNotEqual(self.invoke().returncode,0)
suite=unittest.TestSuite(NativeAudit(n) for n in vars(NativeAudit) if n.startswith('test_native_'));result=unittest.TextTestRunner(verbosity=2).run(suite);raise SystemExit(not result.wasSuccessful())
