import datetime,hashlib,importlib.util,json,os,pathlib,subprocess,sys,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('dispatch',ROOT/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D)

class Fixture(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=pathlib.Path(self.tmp.name);self.clock=0;self.calls=[]
  self.goals=[]
  for name in ['scroll','ink']:
   root=self.root/name;repo=root/'repository';repo.mkdir(parents=True)
   self.goals.append({'id':name,'root':str(root),'repository':str(repo),'cycle_command':['/usr/bin/python3','/fake-cycle',str(root),'--one-cycle'],'between_cycles_seconds':0,'acceptance':{'completion_checks':[],'required_evidence':['scientific_goal']}})
  self.m={'goals':self.goals,'worker_slots':1,'pool_lock':str(self.root/'pool.lock'),'controller':'/fake-controller','state':str(self.root/'state.json')}
  self.path=self.root/'state.json'
 def fake(self,goal,record,status='accepted'):
  self.calls.append(goal['id']);root=pathlib.Path(goal['root']);seq=record['expected_sequence'];result=root/('result-'+str(seq)+'.json')
  out={'goal_id':goal['id'],'sequence':seq,'status':status,'cleanup_confirmed':True,'model':'opencode/big-pickle','job_id':goal['id']+'-'+str(seq),'usage':{'cost':0,'tokens':{'input':9,'output':2,'cache':{'read':17,'write':None}}},'recovery':{'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}}
  result.write_text(json.dumps(out));D.save(root/'status.json',{'sequence':seq,'current':None,'status':'waiting','last_cycle':{'sequence':seq,'result':str(result)}})
 def audit(self,g,out):
  if out['status']!='accepted':return None
  return {'job':out['job_id'],'commit':'a'*40,'receipt':'fixture-'+out['job_id'],'result':out['_result_path']}
 def manager(self,**kwargs):return D.Dispatcher(self.m,self.path,runner=kwargs.pop('runner',self.fake),audit=kwargs.pop('audit',self.audit),complete=kwargs.pop('complete',lambda g,m:{'passed':False,'status':'awaiting_acceptance'}),clock=lambda:self.clock,**kwargs)
 def test_fair_milestone_scheduling_and_no_success_from_patch_or_prose(self):
  d=self.manager();[d.step() for _ in range(4)];self.assertEqual(self.calls,['scroll','ink','scroll','ink']);self.assertTrue(all(s['state']=='ready' for s in d.s['goals'].values()));self.assertIsNone(d.s['goals']['ink']['usage']['tokens']['cache']['write'])
 def test_reopen_continues_cursor_without_duplicate_dispatch(self):
  d=self.manager();d.step();d=self.manager();d.recover();d.step();self.assertEqual(self.calls,['scroll','ink'])
 def test_recover_completed_host_handoff_uses_existing_result_not_new_cycle(self):
  d=self.manager();g=self.goals[0];record={'goal':g['id'],'index':0,'expected_sequence':1,'phase':'running'};d.s['current']=record;d.persist();self.fake(g,record);self.calls=[];d=self.manager();d.recover();self.assertEqual(self.calls,[]);self.assertEqual(d.s['cursor'],1);self.assertEqual(len(d.s['goals']['scroll']['milestones']),1)
 def test_uncertain_restart_is_fenced_without_dispatch(self):
  d=self.manager();d.s['current']={'goal':'scroll','index':0,'expected_sequence':1,'phase':'spawning'};d.persist();d=self.manager()
  with self.assertRaisesRegex(RuntimeError,'uncertain'):d.recover()
  self.assertEqual(self.calls,[])
 def test_live_owner_is_not_replaced(self):
  d=self.manager();p=D.identity(os.getpid());d.s['current']={'goal':'scroll','index':0,'expected_sequence':1,'process':{k:v for k,v in p.items() if k!='argv'}};d.persist()
  with self.assertRaisesRegex(RuntimeError,'alive'):self.manager().recover()
 def test_global_and_goal_locks_refuse_duplicates_and_survive_parent_fd_close(self):
  locks=D.Locks();locks.acquire(self.root/'lock');self.addCleanup(locks.close);other=D.Locks();self.addCleanup(other.close)
  with self.assertRaises(RuntimeError):other.acquire(self.root/'lock')
  p=subprocess.Popen(['/usr/bin/python3','-c','import time;time.sleep(.2)'],pass_fds=locks.fds());locks.close()
  with self.assertRaises(RuntimeError):other.acquire(self.root/'lock')
  p.wait();other.acquire(self.root/'lock')
 def test_goal_scope_roots_and_policy_changes_fail_closed(self):
  d=self.manager();self.m['goals'][1]['repository']=self.goals[0]['repository']
  with self.assertRaises(ValueError):self.manager()
 def test_shutdown_is_explicit_not_elapsed_and_command_uses_actual_repo_cwd(self):
  g=self.goals[0];root=pathlib.Path(g['root']);script=root/'fake.py';script.write_text('import os,pathlib,sys,time\npathlib.Path("observed").write_text(os.getcwd()+"\\n"+repr(sys.argv))\ntime.sleep(.03)\n')
  g['cycle_command']=['/usr/bin/python3',str(script),str(root),'--one-cycle'];d=self.manager();d.pool_fds=();record={'expected_sequence':1};d.run_cycle(g,record);self.assertIn(g['repository'],(pathlib.Path(g['repository'])/'observed').read_text());self.assertIn('--one-cycle',(pathlib.Path(g['repository'])/'observed').read_text())
 def test_all_free_unavailable_waits_but_other_goal_runs(self):
  def fake(g,r):
   self.fake(g,r,'all_free_unavailable' if g['id']=='scroll' else 'accepted')
   if g['id']=='scroll':
    p=pathlib.Path(g['root'])/'status.json';s=json.loads(p.read_text());s['next_check_utc']=datetime.datetime.fromtimestamp(100,datetime.timezone.utc).isoformat();D.save(p,s)
  d=self.manager(runner=fake);d.step();d.step();d.step();self.assertEqual(self.calls,['scroll','ink','ink']);self.clock=100;d.step();self.assertEqual(self.calls[-1],'scroll')
 def test_verified_success_stops_only_that_goal_and_completion_requires_receipts(self):
  d=self.manager(complete=lambda g,m:{'passed':True,'status':'passed','commit':m['commit'],'receipts':[{'fixture_only':True}]} if g['id']=='scroll' else {'passed':False,'status':'awaiting_acceptance'});d.step();d.step();d.step();self.assertEqual(self.calls,['scroll','ink','ink']);self.assertEqual(d.s['goals']['scroll']['state'],'complete')
 def test_fake_success_without_receipts_does_not_stop_goal(self):
  d=self.manager(complete=lambda g,m:{'passed':True,'status':'passed','commit':m['commit']});d.step();self.assertEqual(d.s['goals']['scroll']['state'],'ready')
 def test_scope_uncertainty_and_policy_violation_pause_dispatch(self):
  d=self.manager(audit=lambda g,o:(_ for _ in ()).throw(RuntimeError('Unconfirmed stop')))
  with self.assertRaisesRegex(RuntimeError,'Unconfirmed'):d.step()
  self.assertEqual(d.s['status'],'needs_attention');self.assertIsNotNone(d.s['current'])
 def test_no_twenty_minute_or_eight_hour_age_stop(self):
  d=self.manager();d.step();self.clock=49*3600;d.step();d.step();self.assertEqual(self.calls,['scroll','ink','scroll'])
 def test_real_independent_completion_runs_in_clean_checkout_and_tamper_is_detected(self):
  g=self.goals[0];repo=pathlib.Path(g['repository'])
  for argv in [['git','init','-q',str(repo)],['git','-C',str(repo),'add','.']]:
   if argv[-1]=='.':(repo/'proof').write_text('real fixture artifact')
   subprocess.run(argv,check=True,stdout=subprocess.DEVNULL)
  subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','fixture'],check=True)
  commit=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip()
  check=self.root/'trusted-goal-check.py';check.write_text("import os,json,pathlib\nassert pathlib.Path('proof').read_text()=='real fixture artifact'\nprint(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'fixture_goal'}))\n")
  g['acceptance']={'required_evidence':['fixture_goal'],'completion_checks':[{'id':'independent-fixture','argv':['/usr/bin/python3',str(check)],'evidence_type':'fixture_goal'}]}
  def audit(goal,out):return {'job':out['job_id'],'commit':commit,'receipt':'fake-provider-controller-receipt','result':out['_result_path']}
  d=self.manager(audit=audit,complete=None)
  # Use the real clean-checkout acceptance implementation rather than the default stub.
  d.complete=d.complete_goal;d.step();gs=d.s['goals']['scroll'];self.assertEqual(gs['state'],'complete')
  receipt=gs['goal_acceptance']['receipts'][0];self.assertEqual(receipt['exit_status'],0)
  pathlib.Path(receipt['stdout']).write_text('forged')
  with self.assertRaisesRegex(RuntimeError,'changed'):d.audit_completed()
 def test_pause_before_admission_does_not_starve_other_goal_or_dispatch_again(self):
  def fake(g,r):
   if g['id']=='scroll':D.save(pathlib.Path(g['root'])/'status.json',{'sequence':0,'current':None,'status':'paused','pause_reason':'Operator PAUSE file'})
   else:self.fake(g,r)
  d=self.manager(runner=fake);d.step();d.step();self.assertEqual(self.calls,['ink']);self.assertEqual(d.s['goals']['scroll']['state'],'paused')
 def test_restart_audits_old_milestones_even_when_goal_is_not_complete(self):
  d=self.manager();d.step();d=self.manager(audit=lambda g,o:(_ for _ in ()).throw(RuntimeError('archive missing')))
  with self.assertRaisesRegex(RuntimeError,'archive missing'):d.audit_completed()
 def test_shared_quota_circuits_survive_restart_and_reach_other_goal(self):
  self.m['circuits_file']=str(self.root/'circuits.json')
  def fake(g,r):
   if g['id']=='ink':self.assertEqual(json.loads((self.root/'circuits.json').read_text())['opencode/big-pickle']['kind'],'unavailable')
   self.fake(g,r)
   if g['id']=='scroll':
    p=pathlib.Path(g['root'])/'status.json';status=json.loads(p.read_text());status['circuits']={'opencode/big-pickle':{'kind':'unavailable','until':999999}};D.save(p,status)
  d=self.manager(runner=fake);d.step();d=self.manager(runner=fake);d.step()
 def test_trusted_byte_change_rejected_before_dispatch(self):
  trusted=self.root/'trusted';trusted.write_text('frozen');self.m['trusted_files']={str(trusted):hashlib.sha256(trusted.read_bytes()).hexdigest()};d=self.manager();trusted.write_text('changed')
  with self.assertRaisesRegex(ValueError,'bytes changed'):self.manager()
  self.assertEqual(self.calls,[])
 def test_manifest_policy_change_rejected_after_restart(self):
  d=self.manager();self.m['goals'][0]['acceptance']['required_evidence']=['weaker']
  with self.assertRaisesRegex(ValueError,'migration'):self.manager()
 def test_restart_audit_failure_is_durable_needs_attention(self):
  d=self.manager();d.step();d=self.manager(audit=lambda g,o:(_ for _ in ()).throw(RuntimeError('forged receipt')))
  with self.assertRaisesRegex(RuntimeError,'forged'):d.recover_and_audit()
  self.assertEqual(json.loads(self.path.read_text())['status'],'needs_attention')
 def test_owner_sigterm_waits_for_owned_child_cleanup_and_fences_restart(self):
  g=self.goals[0];script=self.root/'waiting-cycle.py';ready=self.root/'ready';stopped=self.root/'stopped'
  script.write_text("import pathlib,signal,time,sys\n"+f"def stop(*a):pathlib.Path({str(stopped)!r}).write_text('stopped');sys.exit(0)\n"+f"signal.signal(signal.SIGTERM,stop)\npathlib.Path({str(ready)!r}).write_text('ready')\nwhile True:time.sleep(.01)\n")
  g['cycle_command']=[sys.executable,str(script)];self.m['trusted_files']={str(script):hashlib.sha256(script.read_bytes()).hexdigest()};manifest=self.root/'manifest.json';manifest.write_text(json.dumps(self.m))
  p=subprocess.Popen([sys.executable,str(ROOT/'research-dispatch.py'),'run','--manifest',str(manifest)],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
  try:
   until=time.monotonic()+5
   while not ready.exists() and time.monotonic()<until:time.sleep(.01)
   self.assertTrue(ready.exists());p.terminate();out=p.communicate(timeout=5);self.assertEqual(p.returncode,0,out);self.assertTrue(stopped.exists())
   d=self.manager()
   with self.assertRaisesRegex(RuntimeError,'uncertain'):d.recover()
  finally:
   if p.poll() is None:p.kill();p.wait()
 def test_rejected_candidate_and_inconclusive_science_continue_fairly_until_verified_win(self):
  def fake(g,r):
   self.fake(g,r,'failed' if r['expected_sequence']==1 else 'accepted')
  d=self.manager(runner=fake)
  for _ in range(6):d.step()
  self.assertEqual(self.calls,['scroll','ink','scroll','ink','scroll','ink'])
  self.assertTrue(all(s['state']=='ready' for s in d.s['goals'].values()))
  self.assertTrue(all(len(s['milestones'])==2 for s in d.s['goals'].values()))
 def test_explicit_implementation_migration_keeps_goals_and_acceptance_immutable(self):
  d=self.manager();old=json.loads(json.dumps(self.m));pin=self.root/'pin';pin.write_text('reviewed implementation');self.m['controller']='/repaired-controller';self.m['trusted_files']={str(pin):hashlib.sha256(pin.read_bytes()).hexdigest()}
  migration=self.root/'migration.json';body={'kind':'lifecycle-recovery-repair','from':D.digest(old),'to':D.digest(self.m),'previous_manifest':old,'reason':'Approved lifecycle repair'};migration.write_text(json.dumps(body));D.migrate_launch_policy(self.m,self.path,migration);self.manager()
  altered=json.loads(json.dumps(self.m));altered['goals'][0]['acceptance']['required_evidence']=[];body.update(previous_manifest=self.m,**{'from':D.digest(self.m),'to':D.digest(altered)});migration.write_text(json.dumps(body))
  with self.assertRaisesRegex(ValueError,'cannot change'):D.migrate_launch_policy(altered,self.path,migration)
 def test_failed_attempt_stop_claim_must_match_real_controller_observation(self):
  g=self.goals[0];out={'goal_id':'scroll','status':'failed','cleanup_confirmed':True,'job_id':'stopped','_result_path':str(self.root/'result.json'),'recovery':{'stop_id':'claimed-stop','execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}}
  controller=self.root/'inspector';controller.write_text('#!/usr/bin/python3\nimport json\nprint(json.dumps({"execution_recovery":{"stop_id":"different-stop","execution_stop":{"status":"confirmed"},"accounting":{"status":"settled_zero"}}}))\n');controller.chmod(0o700);self.m['controller']=str(controller);d=self.manager()
  with self.assertRaisesRegex(RuntimeError,'exact failed-attempt stop'):d.audit_cycle(g,out)
if __name__=='__main__':unittest.main(verbosity=2)
