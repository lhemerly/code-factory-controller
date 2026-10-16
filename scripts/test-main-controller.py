"""Real controller acceptance/receipt checks; GitHub and coding are fakes."""
import importlib.util,json,pathlib,unittest
S=pathlib.Path(__file__).resolve().parent
def load(name,file):
 spec=importlib.util.spec_from_file_location(name,S/file);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
suite=load('controller_suite','test-github-controller.py');pubsuite=load('publication_suite','test-main-publication.py');M=pubsuite.M
class Boundary(unittest.TestCase):
 def setUp(self):
  self.fixture=suite.ControllerBoundary('test_actual_receipt_publishes_and_restart_deduplicates');self.fixture.setUp();self.addCleanup(self.fixture.tearDown);f=self.fixture
  cfg=suite.M.read(f.goal/'supervisor.json');cfg['controller_sha256']=M.sha(cfg['controller']);suite.M.save(f.goal/'supervisor.json',cfg)
  self.goal=suite.M.read(f.goal/'goal.json');self.goal.update(root=str(f.goal),repository=str(f.goal/'repository'));out=suite.M.read(f.result)
  self.b={'repository':'lhemerly/ink-detector','repository_id':1422424015,'private':True,'branch':'main','remote_url':'git@github.com:lhemerly/ink-detector.git','initial_head':out['base'],'goal_id':self.goal['id'],'goal_root':str(f.goal),'allowed_paths':['result.txt'],'protected_paths':['README.md'],'forbidden_patterns':['private-runtime-value'],'required_checks':[]}
  self.remote=pubsuite.Remote(self.b,out['base']);self.p=M.Publisher(f.root/'main-state',self.remote)
 def test_real_controller_receipt_and_archive_allow_one_direct_publication(self):
  self.assertEqual(self.p.publish(self.b,self.goal)['state'],'confirmed');self.p.publish(self.b,self.goal);self.assertEqual(len(self.remote.pushes),1)
 def test_real_whole_goal_receipt_waits_for_ci_and_replays_publication_once(self):
  f=self.fixture;out=suite.M.read(f.result);commit=out['acceptance']['integrated_commit'];cfg=suite.M.read(f.goal/'supervisor.json')
  code="import json,os,pathlib;assert pathlib.Path('result.txt').read_text()=='verified result\\n';print(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'runtime_tests'}))"
  self.goal['acceptance']={'required_evidence':['runtime_tests'],'completion_checks':[{'id':'whole-goal','evidence_type':'runtime_tests','argv':['/usr/bin/python3','-c',code]}]}
  result=self.p.D.acceptance.run_acceptance({**self.goal['acceptance'],'repository':self.goal['repository'],'verifier_executable':cfg['controller']},f.root/'whole-goal',commit)
  self.assertTrue(result['passed'],result);self.assertTrue(result['receipts'][0]['executor']['quiescent'])
  suite.M.save(f.goal/'goal-evaluation.json',{'goal_id':self.goal['id'],'commit':commit,'result':result})
  self.b.update(require_goal_completion=True,preflight_branch_prefix='factory/verified',required_checks=['controller'])
  self.remote=pubsuite.PreflightRemote(self.b,out['base']);self.p=M.Publisher(f.root/'main-state',self.remote)
  self.assertEqual(self.p.publish(self.b,self.goal)['state'],'awaiting_ci');self.assertEqual(self.remote.pushes,[])
  self.p=M.Publisher(f.root/'main-state',self.remote)
  self.remote.checks=[{'name':'controller','status':'completed','conclusion':'success'}]
  self.assertEqual(self.p.publish(self.b,self.goal)['state'],'confirmed');self.p.publish(self.b,self.goal)
  self.assertEqual(self.remote.pushes,[commit]);self.assertEqual(len(self.remote.preflight_pushes),1)
  pathlib.Path(result['receipts'][0]['stdout']).write_text('tampered')
  with self.assertRaises(ValueError):self.p.publish(self.b,self.goal)
  self.assertEqual(len(self.remote.pushes),1)
 def test_forged_receipt_refused_before_publication(self):
  f=self.fixture;out=suite.M.read(f.result);out['acceptance']['active_verification_receipt']['verification_id']='forged';suite.M.save(f.result,out)
  with self.assertRaises(RuntimeError):self.p.publish(self.b,self.goal)
  self.assertEqual(self.remote.pushes,[])
 def test_corrupt_delivered_archive_refused(self):
  f=self.fixture;out=suite.M.read(f.result);p=pathlib.Path(out['acceptance']['delivery']['destination']);p.write_bytes(p.read_bytes()+b'corrupt')
  with self.assertRaises(RuntimeError):self.p.publish(self.b,self.goal)
  self.assertEqual(self.remote.pushes,[])
 def test_other_goal_or_active_worker_refused(self):
  with self.assertRaises(ValueError):self.p.publish(self.b,{**self.goal,'id':'other'})
  f=self.fixture;s=suite.M.read(f.goal/'status.json');s['current']={'owned':True};suite.M.save(f.goal/'status.json',s)
  with self.assertRaises(ValueError):self.p.publish(self.b,self.goal)
  self.assertEqual(self.remote.pushes,[])
if __name__=='__main__':unittest.main()
