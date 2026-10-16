"""Real existing Factory process supervisor; no providers or shared runtime."""
import importlib.util,json,os,pathlib,subprocess,sys,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('acceptance',ROOT/'goal-acceptance.py');A=importlib.util.module_from_spec(spec);spec.loader.exec_module(A)
BINARY=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',ROOT.parent/'target/debug/code-factory-controller')).resolve()

class Lifecycle(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=pathlib.Path(self.tmp.name);self.repo=self.root/'repo';self.repo.mkdir()
  subprocess.run(['git','init','-q',str(self.repo)],check=True)
  subprocess.run(['git','-C',str(self.repo),'-c','user.name=Test','-c','user.email=test@localhost','commit','--allow-empty','-qm','baseline'],check=True)
  self.commit=subprocess.check_output(['git','-C',str(self.repo),'rev-parse','HEAD'],text=True).strip()
  self.m={'repository':str(self.repo),'verifier_executable':str(BINARY),'required_evidence':['goal_tests'],'completion_checks':[{'id':'goal','evidence_type':'goal_tests','argv':[]} ]}
 def run_code(self,code,timeout=5):
  self.m['completion_checks'][0].update(argv=[sys.executable,'-c',code],timeout_seconds=timeout)
  return A.run_acceptance(self.m,self.root/'state',self.commit)
 def evidence(self):
  return "print(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'goal_tests'}))"
 def test_quiescent_success_preserves_only_approved_environment(self):
  self.m['verification_environment']={'CARGO_HOME':'approved-goal-home'}
  r=self.run_code("import os,json;assert os.environ['CARGO_HOME']=='approved-goal-home';assert 'FACTORY_OPERATOR_TOKEN' not in os.environ;"+self.evidence())
  self.assertTrue(r['passed'],r);p=r['receipts'][0]['executor'];self.assertTrue(p['quiescent']);self.assertFalse(p['leaked_descendants']);self.assertEqual(p['observer'],'linux_subreaper_waitpid_pidfd')
 def test_detached_child_cannot_pass_and_is_reaped(self):
  pid=self.root/'child.pid'
  code="import subprocess,sys,pathlib,json,os;p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True);pathlib.Path("+repr(str(pid))+").write_text(str(p.pid));"+self.evidence()
  r=self.run_code(code);self.assertFalse(r['passed'],r);p=r['receipts'][0]['executor'];self.assertTrue(p['quiescent']);self.assertTrue(p['leaked_descendants']);self.assertGreaterEqual(p['descendants_reaped'],1);self.assertFalse(pathlib.Path('/proc',pid.read_text()).exists())
 def test_timeout_cleans_owned_work_and_retains_receipt(self):
  r=self.run_code('import time;time.sleep(30)',1);self.assertFalse(r['passed']);self.assertEqual(r['receipts'][0]['executor']['termination_reason'],'timeout');self.assertTrue(r['receipts'][0]['executor']['quiescent']);self.assertFalse(r['execution_unresolved'])
 def test_fast_oversized_output_is_rejected_after_exit(self):
  r=self.run_code("print('x'*1100000)");self.assertFalse(r['passed']);self.assertTrue(r['receipts'][0]['executor']['output_truncated']);self.assertTrue(r['receipts'][0]['executor']['quiescent'])
 def test_missing_process_receipt_is_unresolved_without_fallback(self):
  self.m['verifier_executable']='/usr/bin/true'
  r=self.run_code("import pathlib;pathlib.Path('should-not-run').write_text('bad')")
  self.assertFalse(r['passed']);self.assertTrue(r['execution_unresolved']);self.assertFalse((self.repo/'should-not-run').exists())
 def test_owner_loss_closes_control_pipe_and_reaps_detached_child(self):
  pid=self.root/'child.pid'
  child="import subprocess,sys,pathlib,time;p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True);pathlib.Path("+repr(str(pid))+").write_text(str(p.pid));time.sleep(30)"
  self.m['completion_checks'][0].update(argv=[sys.executable,'-c',child],timeout_seconds=30)
  driver="import importlib.util,json,pathlib;spec=importlib.util.spec_from_file_location('a',"+repr(str(ROOT/'goal-acceptance.py'))+");a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a);a.run_acceptance("+repr(self.m)+","+repr(str(self.root/'state'))+","+repr(self.commit)+")"
  p=subprocess.Popen([sys.executable,'-c',driver],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
  try:
   until=time.monotonic()+5
   while not pid.exists() and time.monotonic()<until:time.sleep(.01)
   self.assertTrue(pid.exists());p.terminate();p.communicate(timeout=5)
   until=time.monotonic()+5
   while pathlib.Path('/proc',pid.read_text()).exists() and time.monotonic()<until:time.sleep(.01)
   self.assertFalse(pathlib.Path('/proc',pid.read_text()).exists())
  finally:
   if p.poll() is None:p.kill();p.communicate(timeout=5)
 def test_live_prior_verifier_blocks_replacement_before_launch(self):
  state=self.root/'state';directory=pathlib.Path(str(state)+'.acceptance')/'pending';directory.mkdir(parents=True)
  (directory/'0.plan.json').write_text(json.dumps({'execution_id':'pending'}))
  r=self.run_code("raise AssertionError('replacement must not launch')")
  self.assertTrue(r['execution_unresolved']);self.assertEqual(r['receipts'],[])
  (directory/'0.process.json').write_text(json.dumps({'version':1,'execution_id':'pending','observer':'linux_subreaper_waitpid_pidfd','quiescent':True}))
  r=self.run_code('import json,os;'+self.evidence());self.assertTrue(r['passed'],r)

if __name__=='__main__':unittest.main(verbosity=2)
