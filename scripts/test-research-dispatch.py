import datetime,hashlib,importlib.util,json,os,pathlib,subprocess,sys,tempfile,time,unittest
from unittest.mock import patch
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('dispatch',ROOT/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D)

class Fixture(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=pathlib.Path(self.tmp.name);self.clock=0;self.calls=[]
  self.goals=[]
  for name in ['scroll','ink']:
   root=self.root/name;repo=root/'repository';repo.mkdir(parents=True)
   self.goals.append({'id':name,'root':str(root),'repository':str(repo),'cycle_command':['/usr/bin/python3','/fake-cycle',str(root),'--one-cycle'],'between_cycles_seconds':0,'acceptance':{'completion_checks':[{'id':'fixture','evidence_type':'scientific_goal','argv':['/usr/bin/true']}],'required_evidence':['scientific_goal']}})
  self.m={'goals':self.goals,'worker_slots':1,'pool_lock':str(self.root/'pool.lock'),'controller':'/fake-controller','state':str(self.root/'state.json')}
  self.path=self.root/'state.json'
 def fake(self,goal,record,status='accepted'):
  self.calls.append(goal['id']);root=pathlib.Path(goal['root']);seq=record['expected_sequence'];result=root/('result-'+str(seq)+'.json')
  out={'goal_id':goal['id'],'sequence':seq,'status':status,'cleanup_confirmed':True,'model':'opencode/big-pickle','job_id':goal['id']+'-'+str(seq),'usage':{'cost':0,'tokens':{'input':9,'output':2,'cache':{'read':17,'write':None}}},'recovery':{'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}}
  result.write_text(json.dumps(out));D.save(root/'status.json',{'sequence':seq,'current':None,'status':'waiting','last_cycle':{'sequence':seq,'result':str(result)}})
 def audit(self,g,out):
  if out['status']!='accepted':return None
  return {'job':out['job_id'],'commit':'a'*40,'receipt':'fixture-'+out['job_id'],'result':out['_result_path']}
 def manager(self,**kwargs):return D.Dispatcher(self.m,self.path,runner=kwargs.pop('runner',self.fake),audit=kwargs.pop('audit',self.audit),complete=kwargs.pop('complete',lambda g,m:{'passed':False,'status':'acceptance_failed','phase':'check_evidence'}),clock=lambda:self.clock,**kwargs)
 def test_fair_milestone_scheduling_and_no_success_from_patch_or_prose(self):
  d=self.manager();[d.step() for _ in range(4)];self.assertEqual(self.calls,['scroll','ink','scroll','ink']);self.assertTrue(all(s['state']=='ready' for s in d.s['goals'].values()));self.assertIsNone(d.s['goals']['ink']['usage']['tokens']['cache']['write'])
 def test_reopen_continues_cursor_without_duplicate_dispatch(self):
  d=self.manager();d.step();d=self.manager();d.recover();d.step();self.assertEqual(self.calls,['scroll','ink'])
 def publication_fixture(self,passed=True):
  binding=self.root/'publication-binding.json';D.save(binding,{'require_goal_completion':True});self.m['main_publication']={'scroll':{'binding':str(binding)}}
  # Mocked completion, but retain the complete supported receipt shape so the
  # real restart audit still checks its frozen policy, plan and artifact bytes.
  stdout=self.root/'0.stdout';stderr=self.root/'0.stderr';stderr.write_text('');process=self.root/'0.process.json'
  evidence={'passed':True,'skipped':False,'executed':1,'failed':0,'commit':'a'*40,'evidence_type':'scientific_goal'};D.save(stdout,evidence)
  executor={'version':1,'execution_id':'mock-completion','observer':'linux_subreaper_waitpid_pidfd','root_exit_status':0,'quiescent':True,'leaked_descendants':False,'termination_reason':'root_exit','cleanup_error':None,'output_truncated':False};D.save(process,executor)
  D.save(self.root/'0.plan.json',{'execution_id':'mock-completion','commit':'a'*40,'argv':['/usr/bin/env','CI=true','GIT_CONFIG_NOSYSTEM=1','GIT_CONFIG_GLOBAL=/dev/null','/usr/bin/true'],'seconds':300,'stdout':str(stdout),'log':str(stderr),'receipt':str(process),'cwd':str(self.root/'checkout')})
  result={'passed':passed,'status':'passed' if passed else 'acceptance_failed','commit':'a'*40,'receipts':[{'check_id':'fixture','commit':'a'*40,'exit_status':0,'stdout':str(stdout),'stderr':str(stderr),'stdout_sha256':hashlib.sha256(stdout.read_bytes()).hexdigest(),'stderr_sha256':hashlib.sha256(stderr.read_bytes()).hexdigest(),'evidence':evidence,'executor':executor,'process_receipt':str(process)}]}
  d=self.manager(complete=lambda *_:result);d.publish_goal=lambda g:{'state':'awaiting_ci'};return d,stdout
 def test_publication_wait_restart_does_not_dispatch_another_coding_cycle(self):
  d,_=self.publication_fixture();d.step();self.assertEqual(d.s['goals']['scroll']['state'],'publishing');self.assertEqual(self.calls,['scroll'])
  d=self.manager();d.s['goals']['ink']['state']='paused';d.persist();d.publish_goal=lambda g:{'state':'confirmed'}
  self.clock=30;d.recover_and_audit();d.step();self.assertEqual(self.calls,['scroll']);self.assertEqual(d.s['goals']['scroll']['state'],'complete');self.assertIsNone(d.s['current'])
 def test_publication_crash_reconciles_intent_without_duplicate_coding(self):
  d,_=self.publication_fixture();d.step();d.s['current']={'goal':'scroll','index':0,'phase':'publication'};d.persist()
  d=self.manager();d.publish_goal=lambda g:{'state':'confirmed'};d.recover_and_audit()
  self.assertEqual(self.calls,['scroll']);self.assertEqual(d.s['goals']['scroll']['state'],'complete');self.assertIsNone(d.s['current'])
 def test_failed_whole_goal_never_invokes_publication(self):
  d,_=self.publication_fixture(passed=False);d.publish_goal=lambda g:self.fail('Incomplete goal must not upload');d.step()
  self.assertEqual(d.s['goals']['scroll']['state'],'ready');self.assertEqual(self.calls,['scroll'])
 def test_tampered_goal_evidence_blocks_publication_recovery(self):
  d,stdout=self.publication_fixture();d.step();d.s['current']={'goal':'scroll','index':0,'phase':'publication'};d.persist();stdout.write_text('changed')
  d=self.manager();d.publish_goal=lambda g:self.fail('Tampered evidence must not upload')
  with self.assertRaisesRegex(RuntimeError,'evidence changed'):d.recover_and_audit()
  self.assertEqual(self.calls,['scroll']);self.assertEqual(d.s['status'],'needs_attention')
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
  d=self.manager(complete=lambda g,m:{'passed':True,'status':'passed','commit':m['commit'],'receipts':[{'fixture_only':True}]} if g['id']=='scroll' else {'passed':False,'status':'acceptance_failed','phase':'check_evidence'});d.step();d.step();d.step();self.assertEqual(self.calls,['scroll','ink','ink']);self.assertEqual(d.s['goals']['scroll']['state'],'complete')
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
  d.complete=lambda goal,milestone:D.acceptance.run_acceptance({**goal['acceptance'],'repository':goal['repository']},self.path.parent/'legacy-goal',milestone['commit'])
  d.step();gs=d.s['goals']['scroll'];self.assertEqual(gs['state'],'complete')
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
 def test_confirmed_denial_finishes_audit_and_stays_paused(self):
  audits=[]
  def runner(g,record):
   self.fake(g,record,status='failed');root=pathlib.Path(g['root']);s=json.loads((root/'status.json').read_text())
   p=pathlib.Path(s['last_cycle']['result']);x=json.loads(p.read_text());x.update(phase='native',failure_kind='blocked',error='Permission denied: external_directory',fence_error=None)
   D.save(p,x);s.update(status='paused',pause_reason=x['error']);D.save(root/'status.json',s)
  d=self.manager(runner=runner,audit=lambda g,o:audits.append(o) or None);d.step()
  self.assertEqual(len(audits),1,'The independent controller audit must run before any failover')
  self.assertEqual(d.s['goals']['scroll']['state'],'paused','A positively stopped permission rejection must never become ready')
  self.assertEqual(self.calls,['scroll'])
 def recovery_fixture(self):
  self.goals=self.goals[:1];self.m['goals']=self.goals
  g=self.goals[0];root=pathlib.Path(g['root']);binary=self.root/'controller';binary.write_text('trusted controller fixture');helper=ROOT/'reconcile-stopped-v2.mjs'
  self.m['controller']=str(binary)
  self.m['trusted_files']={str(helper):hashlib.sha256(helper.read_bytes()).hexdigest(),str(binary):hashlib.sha256(binary.read_bytes()).hexdigest()}
  return g,root,helper
 def test_already_stopped_cycle_finishes_audit_without_rerunning_helper(self):
  g,root,helper=self.recovery_fixture();audits=[]
  def runner(g,record):
   self.fake(g,record,status='failed');s=json.loads((root/'status.json').read_text());p=pathlib.Path(s['last_cycle']['result']);x=json.loads(p.read_text())
   x.update(phase='native',failure_kind='transient',error='fetch failed',fence_error=None,recovery={'stop_id':'stop-'+str(x['sequence']),'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}});D.save(p,x)
  original=D.acceptance.supervised_check;D.acceptance.supervised_check=lambda *a,**k:self.fail('A positively stopped cycle must not run the owned helper again')
  try:d=self.manager(runner=runner,audit=lambda g,o:audits.append(o) or None);d.step()
  finally:D.acceptance.supervised_check=original
  self.assertEqual(len(audits),1,'A stopped cycle still requires the normal independent audit')
  self.assertEqual(d.s['goals']['scroll']['state'],'ready');self.assertEqual(self.calls,['scroll'])
 def test_each_owned_helper_receipt_keeps_a_unique_log_and_is_never_overwritten(self):
  g,root,helper=self.recovery_fixture();seen=[]
  def runner(g,record):
   self.calls.append(g['id']);n=record['expected_sequence'];directory=root/'cycles'/('%06d'%n);directory.mkdir(parents=True)
   out={'sequence':n,'goal_id':g['id'],'status':'failed','phase':'native','base':'a'*40,'model':'opencode/big-pickle','job_id':'job-'+str(n),'attempt':'attempt-'+str(n),'child':'native-'+str(n),'worktree':str(directory/'worktree'),'cleanup_confirmed':True,'failure_kind':'transient','error':'fetch failed','fence_error':'stop unavailable'}
   D.save(directory/'result.json',out)
   D.save(root/'status.json',{'sequence':n,'base':'a'*40,'circuits':{},'status':'paused','current':{'sequence':n,'directory':str(directory),'job':out['job_id'],'child':out['child'],'worktree':out['worktree'],'phase':'native'},'last_cycle':{'sequence':n,'status':'failed','result':str(directory/'result.json')}})
  def helper_effect(executable,argv,checkout,env,timeout,stdout,stderr,receipt):
   seen.append(pathlib.Path(stdout));self.assertIn(str(helper),argv);self.assertEqual(env.get('FACTORY_VERIFICATION_COMMIT'),'a'*40)
   pathlib.Path(stdout).write_text('{"reconciled":true}')
   s=json.loads((root/'status.json').read_text());p=pathlib.Path(s['last_cycle']['result']);x=json.loads(p.read_text())
   x.update(fence_error=None,execution_stop='confirmed',recovery={'stop_id':'stop-'+str(x['sequence']),'execution_stop':{'status':'confirmed'},'accounting':{'status':'settled_zero'}})
   target=p.parent/'reconciled-result.json';D.save(target,x);s.update(current=None,status='waiting',last_cycle={'sequence':x['sequence'],'status':'failed','result':str(target)});D.save(root/'status.json',s);return None
  original=D.acceptance.supervised_check;D.acceptance.supervised_check=helper_effect
  try:
   d=self.manager(runner=runner,audit=lambda g,o:None);d.step();self.assertEqual(len(seen),1);self.assertEqual(d.s['goals']['scroll']['state'],'ready')
   d.step();self.assertEqual(len(seen),2);self.assertNotEqual(seen[0],seen[1],'Never overwrite a previous recovery log')
   self.assertEqual(seen[0].read_text(),'{"reconciled":true}','Prior owned-helper logs must be preserved')
  finally:D.acceptance.supervised_check=original
 def test_goal_failure_is_saved_before_next_cycle_and_survives_dispatcher_restart(self):
  result={'passed':False,'status':'acceptance_failed','commit':'a'*40,'reason':'Missing approved behavior','receipts':[]}
  d=self.manager(complete=lambda g,m:result);d.step()
  feedback=json.loads((pathlib.Path(self.goals[0]['root'])/'goal-evaluation.json').read_text())
  self.assertEqual(feedback['result'],result);self.assertEqual(feedback['goal_id'],'scroll')
  self.assertEqual(feedback['milestone_receipt'],'fixture-scroll-1');self.assertEqual(d.s['goals']['scroll']['state'],'ready')
  self.assertEqual(self.manager().s['goals']['scroll']['goal_acceptance'],result)
 def exit_script(self,terminal=True,cleanup=True):
  goal=self.goals[0];script=self.root/'exit-cycle.py';root=pathlib.Path(goal['root'])
  out={'goal_id':goal['id'],'sequence':1,'status':'accepted','cleanup_confirmed':cleanup,'job_id':'saved-before-exit'}
  status={'sequence':1,'current':None,'status':'waiting','last_cycle':{'sequence':1,'result':str(root/'result.json')}}
  script.write_text('import json,pathlib\n'+
   (f'pathlib.Path({str(root/"result.json")!r}).write_text({json.dumps(out)!r})\npathlib.Path({str(root/"status.json")!r}).write_text({json.dumps(status)!r})\n' if terminal else '')+
   'raise SystemExit(7)\n')
  goal['cycle_command']=[sys.executable,str(script)]
  return goal
 def test_nonzero_host_exit_with_terminal_result_is_audited_without_operator_restart(self):
  self.exit_script();d=self.manager();d.runner=d.run_cycle;d.pool_fds=()
  d.step();self.assertIsNone(d.s['current']);self.assertEqual(d.s['goals']['scroll']['state'],'ready')
  self.assertEqual(len(d.s['goals']['scroll']['milestones']),1)
  self.assertEqual(d.s['goals']['scroll']['last_host_exit']['exit_status'],7)
 def test_nonzero_host_exit_without_terminal_evidence_remains_fenced(self):
  self.exit_script(terminal=False);d=self.manager();d.runner=d.run_cycle;d.pool_fds=()
  with self.assertRaisesRegex(RuntimeError,'without a completed result'):d.step()
  saved=json.loads(self.path.read_text());self.assertEqual(saved['status'],'needs_attention');self.assertEqual(saved['current']['exit_status'],7)
 def test_terminal_file_after_host_crash_cannot_bypass_independent_audit(self):
  self.exit_script();d=self.manager(audit=lambda *_:(_ for _ in ()).throw(RuntimeError('Controller receipt not confirmed')));d.runner=d.run_cycle;d.pool_fds=()
  with self.assertRaisesRegex(RuntimeError,'receipt not confirmed'):d.step()
  self.assertEqual(d.s['goals']['scroll']['state'],'paused');self.assertIsNotNone(d.s['current'])
 def test_nonzero_exit_without_positive_cleanup_is_not_recovered(self):
  self.exit_script(cleanup=False);d=self.manager();d.audit=d.audit_cycle;d.runner=d.run_cycle;d.pool_fds=()
  with self.assertRaisesRegex(RuntimeError,'positive process cleanup'):d.step()
  self.assertEqual(d.s['goals']['scroll']['state'],'paused')
 def test_unresolved_independent_verifier_pauses_and_cannot_be_resumed_by_restart(self):
  result={'passed':False,'status':'acceptance_failed','commit':'a'*40,'execution_unresolved':True,'receipts':[]}
  d=self.manager(complete=lambda *_:result);d.step();self.assertEqual(d.s['goals']['scroll']['state'],'paused')
  with self.assertRaisesRegex(RuntimeError,'verifier execution unresolved'):self.manager().recover_and_audit()
 def test_late_supervisor_quiescence_rechecks_same_milestone_without_coding_dispatch(self):
  result={'passed':False,'status':'acceptance_failed','commit':'a'*40,'execution_unresolved':True,'receipts':[]}
  d=self.manager(complete=lambda *_:result);d.step();calls=list(self.calls)
  record=pathlib.Path(str(self.path.parent/'scroll-goal')+'.acceptance')/'owned';record.mkdir(parents=True)
  (record/'0.plan.json').write_text(json.dumps({'execution_id':'owned'}))
  d=self.manager()
  with self.assertRaises(FileNotFoundError):d.recover_and_audit()
  (record/'0.process.json').write_text(json.dumps({'version':1,'execution_id':'owned','observer':'linux_subreaper_waitpid_pidfd','quiescent':True}))
  d=self.manager(complete=lambda g,m:{'passed':False,'status':'acceptance_failed','commit':m['commit'],'receipts':[]})
  d.recover_and_audit();self.assertEqual(self.calls,calls);self.assertEqual(d.s['goals']['scroll']['state'],'ready');self.assertEqual(len(d.s['goals']['scroll']['milestones']),1)
 def test_approved_initial_base_is_evaluated_without_dispatching_coding(self):
  self.goals[0]['initial_base']='a'*40
  d=self.manager(complete=lambda g,m:{'passed':True,'status':'passed','commit':m['commit'],'receipts':[{'fixture_only':True}]})
  d.step();self.assertEqual(self.calls,[]);self.assertEqual(d.s['goals']['scroll']['state'],'complete');self.assertEqual(d.s['goals']['scroll']['milestones'],[]);self.assertEqual(d.s['goals']['scroll']['completion_base'],'a'*40)
 def test_baseline_failure_reaches_first_worker_and_recovery_does_not_duplicate_coding(self):
  g=self.goals[0];g['initial_base']='a'*40;result={'passed':False,'status':'acceptance_failed','commit':'a'*40,'receipts':[]}
  d=self.manager(complete=lambda *_:result);d.s['current']={'goal':g['id'],'index':0,'phase':'baseline_evaluation','commit':'a'*40};d.persist()
  d=self.manager(complete=lambda *_:result);d.recover_and_audit();self.assertEqual(self.calls,[]);self.assertTrue(d.s['goals']['scroll']['baseline_evaluation']['finished'])
  self.assertTrue(json.loads((pathlib.Path(g['root'])/'goal-evaluation.json').read_text())['baseline'])
 def test_baseline_checks_honor_owner_pause_and_invalid_base(self):
  g=self.goals[0];g['initial_base']='a'*40;(pathlib.Path(g['root'])/'PAUSE').write_text('Owner pause')
  d=self.manager(complete=lambda *_:(_ for _ in ()).throw(AssertionError('must not evaluate')));d.step();self.assertEqual(d.s['goals']['scroll']['state'],'paused');self.assertEqual(self.calls,[])
  g['initial_base']='main'
  with self.assertRaisesRegex(ValueError,'exact commit'):self.manager()
 def test_unusable_completion_policy_never_dispatches_continuous_coding(self):
  invalid=[{'completion_checks':[],'required_evidence':['cpu_tests']},
           {'completion_checks':[{'id':'broken','evidence_type':'cpu_tests','argv':[]}],'required_evidence':['cpu_tests']}]
  for index,policy in enumerate(invalid):
   with self.subTest(policy=policy):
    self.path=self.root/('invalid-'+str(index)+'.json');self.goals[0]['acceptance']=policy
    d=self.manager();d.step()
    self.assertEqual(self.calls,[]);self.assertEqual(d.s['goals']['scroll']['state'],'paused')
    self.assertIn('prerequisites unavailable',d.s['goals']['scroll']['reason'])
 def test_post_milestone_prerequisite_errors_pause_instead_of_repairing_configuration(self):
  for index,result in enumerate([{'status':'awaiting_acceptance'},*[{'status':'acceptance_failed','phase':p} for p in ['policy','setup','check_launch']]]):
   with self.subTest(result=result):
    self.path=self.root/('prerequisite-'+str(index)+'.json')
    d=self.manager(complete=lambda *_,r=result:{'passed':False,'reason':'Unavailable verifier',**r})
    d.step();self.assertEqual(d.s['goals']['scroll']['state'],'paused')
    self.assertIn('Unavailable verifier',d.s['goals']['scroll']['reason'])
 def test_bounded_one_cycle_can_still_await_owner_defined_goal_checks(self):
  self.goals[0]['admission_mode']='one_cycle'
  self.goals[0]['acceptance']={'completion_checks':[],'required_evidence':['owner_scope_review']}
  d=self.manager(complete=lambda *_:{'passed':False,'status':'awaiting_acceptance'})
  d.step();self.assertEqual(self.calls,['scroll']);self.assertEqual(d.s['goals']['scroll']['state'],'paused')
 def test_missing_baseline_verifier_prerequisites_pause_before_spending_worker_time(self):
  self.goals[0]['initial_base']='a'*40
  d=self.manager(complete=lambda *_:{'passed':False,'status':'acceptance_failed','phase':'setup','reason':'Missing Git','receipts':[]});d.step()
  self.assertEqual(d.s['goals']['scroll']['state'],'paused');self.assertEqual(self.calls,[]);self.assertIn('Missing Git',d.s['goals']['scroll']['reason'])
 def test_shared_cooldown_is_ingested_before_first_admission(self):
  path=self.root/'circuits.json';self.m['circuits_file']=str(path)
  circuits={'opencode/beam-free':{'kind':'tool_failure','count':1,'until':999999}}
  D.save(path,circuits);d=self.manager();self.assertEqual(d.s['circuits'],circuits);d.step();self.assertEqual(D.queue_api.read(path),circuits)
 def test_shared_cooldown_update_after_startup_reaches_next_worker(self):
  path=self.root/'circuits.json';self.m['circuits_file']=str(path);circuits={'opencode/beam-free':{'kind':'tool_failure','count':1,'until':999999}}
  def run(g,r):self.assertEqual(D.queue_api.read(path),circuits);self.fake(g,r)
  d=self.manager(runner=run);D.save(path,circuits);d.step();self.assertEqual(D.queue_api.read(path),circuits)
 def test_shared_cooldown_does_not_shorten_saved_denial_on_restart(self):
  path=self.root/'circuits.json';self.m['circuits_file']=str(path);denial={'kind':'denied','count':1,'until':9007199254740991}
  D.save(path,{'opencode/big-pickle':denial});d=self.manager();d.step()
  D.save(path,{'opencode/big-pickle':{'kind':'transient','count':1,'until':99}})
  d=self.manager();d.step();self.assertEqual(D.queue_api.read(path)['opencode/big-pickle'],denial)
 def test_shared_cooldown_update_during_cycle_survives_finish(self):
  path=self.root/'circuits.json';self.m['circuits_file']=str(path);circuits={'opencode/beam-free':{'kind':'tool_failure','count':1,'until':999999}}
  def run(g,r):self.fake(g,r);D.save(path,circuits)
  d=self.manager(runner=run);d.step();self.assertEqual(D.queue_api.read(path),circuits)
 def test_malformed_shared_cooldown_fails_before_admission(self):
  path=self.root/'circuits.json';self.m['circuits_file']=str(path);d=self.manager();path.write_text('{')
  with self.assertRaises(json.JSONDecodeError):d.step()
  self.assertEqual(self.calls,[]);self.assertEqual(path.read_text(),'{')
 def stopped_cycle_runner(self,paused):
  def run(g,record):
   self.fake(g,record,status='failed');root=pathlib.Path(g['root']);s=D.queue_api.read(root/'status.json');p=pathlib.Path(s['last_cycle']['result']);x=D.queue_api.read(p)
   x.update(phase='native',failure_kind='blocked' if paused else 'transient',fence_error=None);x['recovery']['stop_id']='exact-stop';D.save(p,x);D.save(p.parent/'controller.json',{'fixture_only':True})
   if paused:s.update(status='paused',pause_reason='Permission denied: external_directory')
   D.save(root/'status.json',s)
  return run
 def stopped_cycle_controller(self,stop_id='exact-stop'):
  return subprocess.CompletedProcess([],0,stdout=json.dumps({'execution_recovery':{'stop_id':stop_id,'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}}))
 def test_already_stopped_denial_is_controller_audited_and_stays_paused(self):
  d=self.manager(runner=self.stopped_cycle_runner(True),audit=lambda g,o:D.Dispatcher.audit_cycle(d,g,o))
  with patch.object(D.subprocess,'run',return_value=self.stopped_cycle_controller()) as inspect,patch.object(D.acceptance,'supervised_check',side_effect=AssertionError('No duplicate recovery helper')):d.step()
  inspect.assert_called_once();self.assertIn('inspect',inspect.call_args.args[0]);self.assertEqual(d.s['goals']['scroll']['state'],'paused');self.assertIn('Permission denied',d.s['goals']['scroll']['reason']);self.assertEqual(self.calls,['scroll'])
 def test_already_stopped_transient_is_controller_audited_before_failover(self):
  d=self.manager(runner=self.stopped_cycle_runner(False),audit=lambda g,o:D.Dispatcher.audit_cycle(d,g,o))
  with patch.object(D.subprocess,'run',return_value=self.stopped_cycle_controller()) as inspect,patch.object(D.acceptance,'supervised_check',side_effect=AssertionError('No duplicate recovery helper')):d.step()
  inspect.assert_called_once();self.assertEqual(d.s['goals']['scroll']['state'],'ready');self.assertEqual(self.calls,['scroll']);self.assertEqual(d.s['goals']['scroll']['native_status'],'failed')
 def test_already_stopped_controller_identity_mismatch_cannot_fail_over(self):
  d=self.manager(runner=self.stopped_cycle_runner(False),audit=lambda g,o:D.Dispatcher.audit_cycle(d,g,o))
  with patch.object(D.subprocess,'run',return_value=self.stopped_cycle_controller('different-stop')),patch.object(D.acceptance,'supervised_check',side_effect=AssertionError('No duplicate recovery helper')),self.assertRaisesRegex(RuntimeError,'exact failed-attempt stop'):d.step()
  self.assertEqual(d.s['goals']['scroll']['state'],'paused');self.assertEqual(d.s['status'],'needs_attention');self.assertEqual(self.calls,['scroll'])
if __name__=='__main__':unittest.main(verbosity=2)
