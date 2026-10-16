"""Actual controller receipts and adapter publication boundary; coding and GitHub are fakes."""
import importlib.util,json,os,pathlib,subprocess,tempfile,unittest
S=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('suite',S/'test-github-review.py');suite=importlib.util.module_from_spec(spec);spec.loader.exec_module(suite);M=suite.M
class ControllerBoundary(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.root=pathlib.Path(self.temp.name);self.goal=self.root/'goal';repo=self.goal/'repository';repo.mkdir(parents=True)
  (repo/'README.md').write_text(json.dumps({'summary':['Product fixture'],'limitations':['Synthetic software proof only.']}))
  for args in [['init','-q'],['add','.'],['-c','core.hooksPath=/dev/null','-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','baseline']]:subprocess.run(['git','-C',str(repo),*args],check=True)
  base=M.git(repo,'rev-parse','HEAD');goal={'id':'review-fixture','objective':'Adapter fixture, no actual research','completion_criteria':['Fixture output'],'pause_conditions':['Uncertain execution']};M.save(self.goal/'goal.json',goal)
  binary=S.parent/'target/debug/code-factory-controller';subprocess.run([os.sys.executable,str(S/'fake-research-cycle.py'),str(self.goal)],check=True,capture_output=True,env={**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary)})
  cfg={'controller':str(binary)};M.save(self.goal/'supervisor.json',cfg);M.save(self.goal/'envelope.json',{'trusted_files':{str(self.goal/'supervisor.json'):M.sha(self.goal/'supervisor.json')}})
  self.result=self.goal/'cycles/1/result.json';out=M.read(self.result);out['base']=base;M.save(self.result,out)
  self.remote=suite.Remote();self.remote.data=suite.snapshot(base);self.adapter=M.Adapter(self.root/'adapter',self.remote);b=suite.binding();b.update(expected_head=base,allowed_paths=['result.txt'],protected_paths=['README.md']);b['publication']['summary_path']='README.md';self.adapter.bind(b);self.key=self.adapter.ingest();s=self.adapter.state();s['batches'][self.key].update(root=str(self.goal),status='queued');self.adapter.persist(s)
 def test_actual_queue_enrollment_is_scoped_and_replay_safe(self):
  seed=self.root/'seed';seed.mkdir();(seed/'trusted').mkdir();(seed/'trusted/check.py').write_text('print("fixture")')
  binary=S.parent/'target/debug/code-factory-controller';cfg={'native':'fixture-only','native_sha256':'fixture-only','controller':str(binary),'controller_sha256':M.sha(binary),'plugin':'fixture-only','plugin_commit':'fixture-only','venv':'fixture-only','path':'/usr/bin','ownership_lease_seconds':3600,'ownership_heartbeat_seconds':60,'integration_ref':'refs/heads/review-fixture','profiles':[{'id':'fixture','version':'1','timeout_seconds':30,'checks':[{'id':'result','argv':['/usr/bin/true']}]}]}
  M.save(seed/'supervisor.json',cfg);M.save(seed/'goal.json',M.read(self.goal/'goal.json'));M.save(seed/'job-template.json',{'allowed_paths':['result.txt'],'protected_paths':['README.md']});M.save(seed/'protected.json',['README.md']);(seed/'verify.py').write_text('')
  b=suite.binding();base=M.git(self.goal/'repository','rev-parse','HEAD');b.update(expected_head=base,execution_seed=str(seed),seed_pins={str(p):M.sha(p) for p in seed.rglob('*') if p.is_file()})
  remote=suite.Remote();remote.data=suite.snapshot(base);adapter=M.Adapter(self.root/'enqueue',remote);adapter.bind(b);key=adapter.ingest()
  existing=self.root/'existing';existing.mkdir();M.save(existing/'supervisor.json',cfg)
  manifest={'goals':[{'id':'existing','root':str(existing),'repository':str(existing/'repository'),'cycle_command':['/usr/bin/python3',str(S/'fake-research-cycle.py'),str(existing),'--one-cycle']}],'controller':str(binary),'trusted_files':{str(binary):M.sha(binary)}};mp=self.root/'manifest.json';M.save(mp,manifest)
  original=M.run
  def transport(argv,**kwargs):
   if argv[:2]==['/usr/bin/git','clone']:argv=[str(self.goal/'repository') if v==b['remote_url'] else v for v in argv]
   return original(argv,**kwargs)
  from unittest.mock import patch
  with patch.object(M,'run',side_effect=transport):first=adapter.enqueue(key,mp,self.root/'q');second=adapter.enqueue(key,mp,self.root/'q')
  self.assertEqual(first,second);envelope=M.read(pathlib.Path(first['root'])/'envelope.json');self.assertEqual(M.read(pathlib.Path(first['root'])/'job-template.json')['allowed_paths'],b['allowed_paths']);self.assertEqual(envelope['goal']['admission_mode'],'one_cycle')
  self.assertEqual(len(M.D.queue_api.Queue(self.root/'q',M.save).snapshot()['goals']),1)
 def tearDown(self):self.temp.cleanup()
 def test_actual_receipt_publishes_and_restart_deduplicates(self):
  result=self.adapter.publish(self.key);self.assertEqual(result['state'],'confirmed');self.adapter.publish(self.key);self.assertEqual(len(self.remote.pushes),1)
 def test_pr_counts_come_from_authoritative_receipt(self):
  out=M.read(self.result);out['acceptance']['active_verification_receipt']['environment']['checks'][0]['observations']={'executed':99999};M.save(self.result,out)
  self.adapter.publish(self.key);self.assertNotIn('99999',self.remote.bodies[-1])
 def test_forged_saved_receipt_refused(self):
  out=M.read(self.result);out['acceptance']['active_verification_receipt']['verification_id']='forged';M.save(self.result,out)
  with self.assertRaises(RuntimeError):self.adapter.publish(self.key)
  self.assertEqual(self.remote.pushes,[])
 def test_corrupt_delivered_archive_refused(self):
  out=M.read(self.result);path=pathlib.Path(out['acceptance']['delivery']['destination']);path.write_bytes(path.read_bytes()+b'tampered')
  with self.assertRaises(RuntimeError):self.adapter.publish(self.key)
  self.assertEqual(self.remote.pushes,[])
 def test_changed_host_configuration_refused(self):
  (self.goal/'supervisor.json').write_text('{}')
  with self.assertRaises(ValueError):self.adapter.publish(self.key)
  self.assertEqual(self.remote.pushes,[])
if __name__=='__main__':unittest.main()
