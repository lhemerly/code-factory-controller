"""Offline queue/boundary fixtures; no inference. Real locks, subprocesses and Unix wakes."""
import copy,fcntl,hashlib,importlib.util,json,os,pathlib,socket,subprocess,sys,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('dispatch',ROOT/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D);Q=D.queue_api
class Tests(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory(prefix='dispatch-q-');self.addCleanup(self.temp.cleanup);self.root=pathlib.Path(self.temp.name);self.queue=Q.Queue(self.root/'queue',D.save)
  self.goals=[];self.code=self.root/'scripts';self.code.mkdir();self.host=self.code/'host.mjs';self.host.write_text('known good')
  for name in ['existing','new']:
   root=self.root/name;repo=root/'repository';repo.mkdir(parents=True)
   goal={'id':name,'root':str(root),'repository':str(repo),'cycle_command':[sys.executable,str(self.host),str(root),'--one-cycle'],'between_cycles_seconds':0,'admission_mode':'one_cycle','acceptance':{'required_evidence':['owner-review'],'completion_checks':[]}}
   self.goals.append(goal)
   files={'goal.json':{'id':name,'objective':'Approved fixture','completion_criteria':['Independent evidence'],'pause_conditions':['Scope violation']},'supervisor.json':{'native':'/pinned/native','native_sha256':'frozen','controller':'/pinned/controller','controller_sha256':'frozen','plugin':'/pinned/plugin','plugin_commit':'frozen','venv':'/venv','path':'/usr/bin','ownership_lease_seconds':3600,'ownership_heartbeat_seconds':60},'task.txt':'Approved implementation','job-template.json':{'allowed_paths':['new']},'protected.json':[],'controller.json':{'repository':str(repo),'policy':{'free_only':True,'spend_ceiling':0},'max_workers':1}}
   for p,v in files.items():(root/p).write_text(json.dumps(v) if not isinstance(v,str) else v)
  self.pin=self.host;self.manifest={'goals':[self.goals[0]],'worker_slots':1,'pool_lock':str(self.root/'pool.lock'),'state':str(self.root/'state.json'),'controller':'/fake-controller','trusted_files':{str(self.pin):Q.sha(self.pin)}};self.path=self.root/'manifest.json';D.save(self.path,self.manifest)
  self.intent={'goal':self.goals[1],'trusted_files':{str(p):Q.sha(p) for p in pathlib.Path(self.goals[1]['root']).iterdir() if p.is_file()}}
  self.lookup=lambda controller,path:{'goal':Q.read(pathlib.Path(path).parent/'goal.json')}
  self.calls=[]
 def manager(self,runner=None):
  return D.Dispatcher(Q.read(self.path),self.manifest['state'],runner=runner or self.run_cycle,audit=lambda g,o:{'job':o['job_id'],'commit':'a'*40,'receipt':'independent-fixture','result':o['_result_path']} if o['status']=='accepted' else None,complete=lambda g,m:{'passed':False,'status':'awaiting_acceptance'})
 def run_cycle(self,goal,record):
  self.calls.append(goal['id']);root=pathlib.Path(goal['root']);result=root/'result.json';D.save(result,{'sequence':record['expected_sequence'],'goal_id':goal['id'],'job_id':goal['id']+'-'+str(record['expected_sequence']),'status':'accepted','usage':{'cost':None,'tokens':{'input':3,'output':2,'cache':{'read':11,'write':None}}}});D.save(root/'status.json',{'sequence':record['expected_sequence'],'current':None,'status':'waiting','last_cycle':{'result':str(result)}})
 def enqueue(self):return self.queue.enqueue(self.intent,self.manifest,self.lookup)
 def update(self):
  self.manager();candidate=self.root/'candidate';candidate.write_text('reviewed replacement');new=copy.deepcopy(self.manifest);new['trusted_files'][str(self.pin)]=Q.sha(candidate)
  report=self.root/'tests.json';D.save(report,{'passed':True,'permissions_unchanged':True,'tests':[{'name':'compatibility','passed':True}]})
  plan={'previous_manifest':self.manifest,'manifest':new,'files':[{'source':str(candidate),'destination':str(self.pin),'sha256':Q.sha(candidate)}],'verification':{'path':str(report),'sha256':Q.sha(report)}};self.queue.stage(plan);return plan
 def test_registered_is_not_queued_and_duplicate_enqueue_is_idempotent(self):
  self.assertEqual(self.queue.describe({}),[]);a=self.enqueue();self.assertEqual(a,self.enqueue());self.assertEqual(len(self.queue.snapshot()['goals']),1);self.assertEqual(self.queue.describe({})[0]['state'],'queued')
 def test_changed_scope_duplicate_and_unregistered_goal_refused(self):
  self.enqueue();changed=copy.deepcopy(self.intent);changed['goal']['acceptance']['required_evidence']=[]
  with self.assertRaisesRegex(ValueError,'different approved scope'):self.queue.enqueue(changed,self.manifest,self.lookup)
  with self.assertRaisesRegex(ValueError,'registered'):self.queue.enqueue(self.intent,self.manifest,lambda *a:{'goal':{}})
 def test_changed_runtime_or_executor_and_missing_pins_are_refused(self):
  changed=copy.deepcopy(self.intent);changed['goal']['cycle_command'][0]='/unapproved'
  with self.assertRaisesRegex(ValueError,'runner'):self.queue.enqueue(changed,self.manifest,self.lookup)
  changed=copy.deepcopy(self.intent);changed['trusted_files']={}
  with self.assertRaisesRegex(ValueError,'pin'):self.queue.enqueue(changed,self.manifest,self.lookup)
  cfg=pathlib.Path(self.goals[1]['root'])/'supervisor.json';v=Q.read(cfg);v['plugin']='/unapproved';D.save(cfg,v);self.intent['trusted_files'][str(cfg)]=Q.sha(cfg)
  with self.assertRaisesRegex(ValueError,'runtime'):self.enqueue()
 def test_goal_enrollment_preserves_original_policy_and_runs_once_not_complete(self):
  d=self.manager();d.s['goals']['existing']['state']='paused';d.persist();self.enqueue();Q.enroll(self.queue,d,self.path,self.lookup)
  self.assertEqual(d.m['goals'][0],self.manifest['goals'][0]);self.assertEqual(self.queue.describe(d.s)[0]['state'],'queued');d.step()
  self.assertEqual(self.calls,['new']);self.assertEqual(self.queue.describe(d.s)[0]['state'],'awaiting_review');self.assertIsNone(d.s['goals']['new']['usage']['cost']);self.assertIsNone(d.s['goals']['new']['usage']['tokens']['cache']['write'])
  self.assertFalse(d.step());d=self.manager();Q.enroll(self.queue,d,self.path,self.lookup);self.assertFalse(d.step());self.assertEqual(self.calls,['new'])
 def test_running_and_completed_use_existing_dispatcher_state_not_queue_receipts(self):
  self.enqueue();d=self.manager();Q.enroll(self.queue,d,self.path,self.lookup);d.s['current']={'goal':'new'};self.assertEqual(self.queue.describe(d.s)[0]['state'],'running');d.s['current']=None;d.s['goals']['new']['state']='complete';self.assertEqual(self.queue.describe(d.s)[0]['state'],'completed')
 def test_active_execution_rejects_enrollment_and_overlap(self):
  d=self.manager();self.enqueue();d.s['current']={'goal':'existing'}
  with self.assertRaisesRegex(RuntimeError,'active'):Q.enroll(self.queue,d,self.path,self.lookup)
  d.s['current']=None
  # Alias the existing manifest root into the queued root to exercise scope overlap without changing native root semantics.
  d.m['goals'][0]['repository']=self.goals[1]['repository']
  with self.assertRaisesRegex(ValueError,'overlaps'):Q.enroll(self.queue,d,self.path,self.lookup)

 def test_concurrent_registration_loses_no_intent_and_deduplicates(self):
  self.manager();input_path=self.root/'input.json';D.save(input_path,self.intent)
  # Separate processes share real intake flock/atomic writes. Use the exact validation lookup fixture.
  code="import importlib.util,json,pathlib,sys; s=importlib.util.spec_from_file_location('d',sys.argv[1]);d=importlib.util.module_from_spec(s);s.loader.exec_module(d);q=d.queue_api.Queue(sys.argv[2],d.save);q.enqueue(json.loads(pathlib.Path(sys.argv[3]).read_text()),json.loads(pathlib.Path(sys.argv[4]).read_text()),lambda c,p:{'goal':d.queue_api.read(pathlib.Path(p).parent/'goal.json')})"
  ps=[subprocess.Popen([sys.executable,'-c',code,str(ROOT/'research-dispatch.py'),str(self.queue.root),str(input_path),str(self.path)]) for _ in range(6)]
  self.assertTrue(all(p.wait(timeout=10)==0 for p in ps));self.assertEqual(len(self.queue.snapshot()['goals']),1)
 def test_kernel_slot_wait_blocks_without_preemption_and_wakes_on_release(self):
  locks=D.Locks();locks.acquire(self.manifest['pool_lock']);ready=self.root/'ready';done=self.root/'done'
  code="import fcntl,pathlib,sys; f=open(sys.argv[1],'a');pathlib.Path(sys.argv[2]).write_text('waiting');fcntl.flock(f,fcntl.LOCK_EX);pathlib.Path(sys.argv[3]).write_text('acquired')"
  p=subprocess.Popen([sys.executable,'-c',code,self.manifest['pool_lock'],str(ready),str(done)]);self.addCleanup(lambda:p.kill() if p.poll() is None else None)
  limit=time.monotonic()+3
  while not ready.exists() and time.monotonic()<limit:time.sleep(.01)
  self.assertTrue(ready.exists());self.assertFalse(done.exists());self.assertIsNone(p.poll());locks.close();self.assertEqual(p.wait(timeout=3),0);self.assertTrue(done.exists())
 def test_idle_wakeup_is_socket_event_and_pending_intent_survives_missing_receiver(self):
  self.enqueue();channel=self.queue.receiver();self.addCleanup(channel.close);self.queue.notify();channel.settimeout(1);self.assertEqual(channel.recv(1),b'q');self.assertEqual(len(self.queue.snapshot()['goals']),1)
 def test_boundary_promotes_only_verified_pins_and_is_restart_idempotent(self):
  plan=self.update();Q.boundary_updates(self.queue,self.path,self.manifest['state'],D.Locks);self.assertEqual(self.pin.read_text(),'reviewed replacement');self.assertEqual(Q.read(self.path),plan['manifest']);self.manager();Q.boundary_updates(self.queue,self.path,self.manifest['state'],D.Locks)
  self.assertEqual(len(Q.read(self.manifest['state'])['implementation_migrations']),1)
 def test_update_failure_and_post_promotion_audit_restore_known_good(self):
  self.update()
  with self.assertRaisesRegex(RuntimeError,'compatibility'):
   Q.boundary_updates(self.queue,self.path,self.manifest['state'],D.Locks,audit=lambda p:(_ for _ in ()).throw(RuntimeError('compatibility audit failed')))
  self.assertEqual(self.pin.read_text(),'known good');self.assertEqual(Q.read(self.path),self.manifest);self.manager()
 def test_update_crash_journal_rolls_back_then_retries_without_partial_activation(self):
  self.update()
  with self.assertRaises(SystemExit):Q.boundary_updates(self.queue,self.path,self.manifest['state'],D.Locks,crash=lambda i:(_ for _ in ()).throw(SystemExit(19)))
  self.assertTrue((self.queue.root/'update-journal.json').exists());Q.boundary_updates(self.queue,self.path,self.manifest['state'],D.Locks);self.assertEqual(self.pin.read_text(),'reviewed replacement');self.manager()
 def test_failed_tests_or_broader_permissions_cannot_be_staged(self):
  plan=self.update();bad=copy.deepcopy(plan);bad['manifest']['worker_slots']=2
  with self.assertRaisesRegex(ValueError,'cannot change'):self.queue.stage(bad)
  report=pathlib.Path(plan['verification']['path']);v=Q.read(report);v['permissions_unchanged']=False;D.save(report,v);plan['verification']['sha256']=Q.sha(report)
  with self.assertRaisesRegex(ValueError,'permission tests'):self.queue.stage(plan)
 def test_update_refuses_live_goal_even_if_pool_wrongly_available(self):
  self.update();root=pathlib.Path(self.goals[0]['root']);D.save(root/'status.json',{'current':{'worker':'unknown'}})
  with self.assertRaisesRegex(ValueError,'unresolved'):Q.boundary_updates(self.queue,self.path,self.manifest['state'],D.Locks)
  self.assertEqual(self.pin.read_text(),'known good')
 def test_enrollment_crash_replays_exact_manifest_state_transaction(self):
  self.enqueue();d=self.manager();original=self.queue.save;failed=[False]
  def crash(path,value):
   if pathlib.Path(path)==d.path and not failed[0]:failed[0]=True;raise SystemExit(17)
   return original(path,value)
  self.queue.save=crash
  with self.assertRaises(SystemExit):Q.enroll(self.queue,d,self.path,self.lookup)
  self.queue.save=original;Q.enroll(self.queue,d,self.path,self.lookup);self.assertEqual(len(d.m['goals']),2);self.manager()
 def test_unsafe_failed_cycle_remains_blocked_after_restart_and_is_not_resubmitted(self):
  self.enqueue();d=self.manager();Q.enroll(self.queue,d,self.path,self.lookup);d.s['goals']['existing']['state']='paused';d.s['current']={'goal':'new','index':1,'expected_sequence':1,'phase':'spawning'};d.persist()
  with self.assertRaisesRegex(RuntimeError,'uncertain'):self.manager().recover_and_audit()
  self.assertEqual(self.calls,[]);self.assertEqual(Q.read(d.path)['status'],'needs_attention')
 def test_concurrent_distinct_approvals_are_all_retained(self):
  self.manager();inputs=[]
  for n in range(4):
   root=self.root/('parallel-'+str(n));(root/'repository').mkdir(parents=True)
   for original in pathlib.Path(self.goals[1]['root']).iterdir():
    if original.is_file():(root/original.name).write_bytes(original.read_bytes())
   name='parallel-'+str(n);goal=copy.deepcopy(self.goals[1]);goal.update(id=name,root=str(root),repository=str(root/'repository'));goal['cycle_command']=[str(root) if a==self.goals[1]['root'] else a for a in goal['cycle_command']]
   registered=Q.read(root/'goal.json');registered['id']=name;D.save(root/'goal.json',registered);config=Q.read(root/'controller.json');config['repository']=goal['repository'];D.save(root/'controller.json',config)
   envelope={'goal':goal,'trusted_files':{str(p):Q.sha(p) for p in root.iterdir() if p.is_file()}};path=self.root/('approval-'+str(n)+'.json');D.save(path,envelope);inputs.append(path)
  code="import importlib.util,json,pathlib,sys; s=importlib.util.spec_from_file_location('d',sys.argv[1]);d=importlib.util.module_from_spec(s);s.loader.exec_module(d);q=d.queue_api.Queue(sys.argv[2],d.save);q.enqueue(json.loads(pathlib.Path(sys.argv[3]).read_text()),json.loads(pathlib.Path(sys.argv[4]).read_text()),lambda c,p:{'goal':d.queue_api.read(pathlib.Path(p).parent/'goal.json')})"
  ps=[subprocess.Popen([sys.executable,'-c',code,str(ROOT/'research-dispatch.py'),str(self.queue.root),str(path),str(self.path)]) for path in inputs]
  self.assertTrue(all(p.wait(timeout=10)==0 for p in ps));self.assertEqual(len(self.queue.snapshot()['goals']),4)
  d=self.manager();d.s['goals']['existing']['state']='paused';d.persist();Q.enroll(self.queue,d,self.path,self.lookup)
  expected=[item['input']['goal']['id'] for item in self.queue.snapshot()['goals']]
  for _ in range(4):d.step()
  self.assertEqual(self.calls,expected)
 def test_safe_failure_preserves_accounting_blocks_until_explicit_owner_retry(self):
  self.enqueue();d=self.manager();Q.enroll(self.queue,d,self.path,self.lookup);d.s['goals']['existing']['state']='paused'
  def fail(goal,record):
   self.run_cycle(goal,record);root=pathlib.Path(goal['root']);out=Q.read(root/'result.json');out.update(status='failed',cleanup_confirmed=True,recovery={'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}});D.save(root/'result.json',out)
  d.runner=fail;d.step();self.assertEqual(self.queue.describe(d.s)[0]['state'],'blocked');self.assertIsNone(d.s['goals']['new']['usage']['cost']);self.assertFalse(d.step())
  # Existing owner resume checks confirmed cleanup/stop without converting unknown accounting to zero.
  D.owner_resume(d.m);d.s['goals']['new']['state']='ready';d.runner=self.run_cycle;d.step();self.assertEqual(self.calls,['new','new']);self.assertEqual(self.queue.describe(d.s)[0]['state'],'awaiting_review')
 def test_update_cannot_write_unrelated_files_or_change_goal_configuration(self):
  plan=self.update();target=pathlib.Path(self.goals[0]['root'])/'goal.json';bad=copy.deepcopy(plan);bad['files'][0]['destination']=str(target);bad['manifest']['trusted_files'][str(target)]=bad['files'][0]['sha256']
  with self.assertRaisesRegex(ValueError,'unrelated|permissions'):self.queue.stage(bad)
 def test_runtime_policy_can_reuse_any_existing_approved_project_profile(self):
  # Scientific projects may already have different approved read-only Python environments.
  extra=copy.deepcopy(self.goals[0]);other=self.root/'other-profile';other.mkdir();extra.update(id='other-profile',root=str(other));cfg=Q.read(pathlib.Path(self.goals[0]['root'])/'supervisor.json');cfg['venv']='/already-approved-venv';D.save(other/'supervisor.json',cfg);self.manifest['goals'].append(extra)
  path=pathlib.Path(self.goals[1]['root'])/'supervisor.json';new=Q.read(path);new['venv']=cfg['venv'];D.save(path,new);self.intent['trusted_files'][str(path)]=Q.sha(path)
  self.assertEqual(self.enqueue()['input']['goal']['id'],'new')
  new['native']='/unapproved-native';D.save(path,new);self.intent['trusted_files'][str(path)]=Q.sha(path)
  with self.assertRaisesRegex(ValueError,'complete approved runtime'):self.enqueue()
 def test_idle_owner_checks_current_trusted_bytes_before_next_admission(self):
  d=self.manager();self.enqueue();self.pin.write_text('changed after startup')
  with self.assertRaisesRegex(ValueError,'pin mismatch'):Q.enroll(self.queue,d,self.path,self.lookup)
  self.assertEqual(self.calls,[])
  with self.assertRaisesRegex(ValueError,'pin mismatch'):self.enqueue()
 def failed_goal(self):
  goal=self.goals[1];root=pathlib.Path(goal['root']);repo=root/'repository'
  def git(*a):return subprocess.check_output(['git','-C',str(repo),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false','-c','user.name=Fixture','-c','user.email=fixture@localhost',*a],text=True).strip()
  git('init','-q');(repo/'frozen.txt').write_text('frozen');git('add','.');git('commit','-qm','baseline');base=git('rev-parse','HEAD');(repo/'new.txt').write_text('unaccepted');git('add','.');git('commit','-qm','candidate');candidate=git('rev-parse','HEAD')
  cfg=Q.read(root/'supervisor.json');cfg['initial_base']=base;D.save(root/'supervisor.json',cfg);D.save(root/'job-template.json',{'allowed_paths':['new.txt'],'protected_paths':['frozen.txt']})
  self.intent['trusted_files'].update({str(root/p):Q.sha(root/p) for p in ['supervisor.json','job-template.json']});self.enqueue();d=self.manager();Q.enroll(self.queue,d,self.path,self.lookup)
  receipt={'verification_id':'receipt-failed','candidate_commit':candidate,'outcome':'failed'};recovery={'stop_id':'owned-stop','execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}
  out={'goal_id':goal['id'],'sequence':1,'job_id':'failed-job','status':'failed','cleanup_confirmed':True,'candidate':candidate,'scope_approved':True,'failure_kind':'blocked','candidate_status':{'active_verification_receipt':receipt},'recovery':recovery}
  D.save(root/'failed.json',out);D.save(root/'status.json',{'sequence':1,'current':None,'base':base,'accepted':[],'status':'paused','last_cycle':{'result':str(root/'failed.json')}});d.s['goals']['new'].update(state='paused',native_status='failed');d.s['goals']['existing']['state']='paused';d.persist()
  observed={'candidate_commit':candidate,'active_verification_receipt':receipt,'execution_recovery':recovery};return d,goal,out,observed
 def test_failed_candidate_retry_is_receipt_bound_idempotent_and_unaccepted(self):
  d,g,out,observed=self.failed_goal();inspect=lambda *args:observed
  request=Q.request_retry(self.queue,d.m,'new',inspect,D.Locks);self.assertEqual(request,Q.request_retry(self.queue,d.m,'new',inspect,D.Locks));self.assertEqual(len(self.queue.snapshot()['retries']),1)
  Q.retry_goals(self.queue,d,inspect,D.Locks);status=Q.read(pathlib.Path(g['root'])/'status.json');self.assertEqual(status['base'],out['candidate']);self.assertEqual(status['accepted'],[]);self.assertEqual(d.s['goals']['new']['state'],'ready')
  Q.retry_goals(self.queue,d,inspect,D.Locks);self.assertEqual(len(d.s['queue_retries']),1);self.assertEqual(observed['execution_recovery']['accounting']['status'],'unknown')
 def test_retry_refuses_forged_stale_stop_and_user_pause(self):
  d,g,out,observed=self.failed_goal();inspect=lambda *args:observed;bad=copy.deepcopy(observed);bad['execution_recovery']['stop_id']='forged'
  with self.assertRaisesRegex(ValueError,'exact owned stop'):Q.request_retry(self.queue,d.m,'new',lambda *a:bad,D.Locks)
  pause=pathlib.Path(g['root'])/'PAUSE';pause.write_text('User stop')
  with self.assertRaisesRegex(ValueError,'PAUSE'):Q.request_retry(self.queue,d.m,'new',inspect,D.Locks)
  pause.unlink();Q.request_retry(self.queue,d.m,'new',inspect,D.Locks);out['sequence']=2;D.save(pathlib.Path(g['root'])/'failed.json',out)
  with self.assertRaises(ValueError):Q.retry_goals(self.queue,d,inspect,D.Locks)
 def test_retry_refuses_native_permission_block_without_a_failed_receipt(self):
  d,g,out,observed=self.failed_goal();out['candidate']=None;out['candidate_status']={};D.save(pathlib.Path(g['root'])/'failed.json',out);observed['active_verification_receipt']=None
  with self.assertRaisesRegex(ValueError,'Permission/ownership'):Q.request_retry(self.queue,d.m,'new',lambda *a:observed,D.Locks)
if __name__=='__main__':unittest.main(verbosity=2)
