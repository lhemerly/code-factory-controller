import contextlib,copy,fcntl,hashlib,importlib.util,json,os,pathlib,shutil,sqlite3,subprocess,tempfile,unittest
S=pathlib.Path(__file__).resolve().parent
def load(name,file):
 sp=importlib.util.spec_from_file_location(name,S/file);m=importlib.util.module_from_spec(sp);sp.loader.exec_module(m);return m
A=load('absence_regression','pre-admission-recovery.py');P=load('fixture_policy_regression','queue-fixture-policy.py')

class PreAdmissionRegression(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=pathlib.Path(self.tmp.name);self.dir=self.root/'cycles/000002';self.dir.mkdir(parents=True)
  self.controller=self.root/'controller';self.controller.write_text('#!/usr/bin/python3\nimport sys\nsys.stderr.write("Error: job not found\\n\\nCaused by:\\n    Query returned no rows\\n")\nraise SystemExit(1)\n');self.controller.chmod(0o700)
  self.config={'project_id':'goal','repository':str(self.root/'repository'),'data_dir':str(self.dir/'controller')};(self.dir/'controller').mkdir();(self.dir/'controller/controller.lock').touch()
  self.save(self.root/'goal.json',{'id':'goal'});self.save(self.root/'supervisor.json',{'controller':str(self.controller),'controller_sha256':A.sha(self.controller)})
  self.save(self.dir/'controller.json',self.config);self.out={'goal_id':'goal','sequence':2,'status':'failed','phase':'admission','job_id':'reserved-2','cleanup_confirmed':True,'ended_utc':'now','error':'Error: Project instructions exceed admission budget; no instructions were truncated','fence_error':'Error: Controller cancel: {"error":"job not found"}'};self.save(self.dir/'result.json',self.out)
  with contextlib.closing(sqlite3.connect(self.dir/'controller/controller.sqlite')) as c:
   for table in A.EMPTY:c.execute('CREATE TABLE '+table+' (id TEXT)')
  self.mp=self.root/'manifest.json';self.process=self.root/'owner.process.json';self.plan=self.root/'owner.plan.json'
  self.save(self.process,{'observer':'linux_subreaper_waitpid_pidfd','quiescent':True,'leaked_descendants':False,'cleanup_error':None,'execution_id':'owned','root_exit_status':1})
  self.save(self.plan,{'execution_id':'owned','receipt':str(self.process),'argv':['run','--manifest',str(self.mp)]})
 def save(self,p,d):p.write_text(json.dumps(d))
 def absence(self):return A.absence(self.root,self.dir,self.controller)
 def test_absence_is_not_a_stop_or_billing_receipt(self):
  before=(self.dir/'result.json').read_bytes();p=self.absence();self.assertEqual(p['kind'],'not_admitted');self.assertFalse(p['worker_session_created']);self.assertNotIn('execution_stop',p);self.assertNotIn('accounting',p);self.assertEqual(before,(self.dir/'result.json').read_bytes())
 def test_any_ledger_effect_blocks_absence(self):
  for table in A.EMPTY:
   with self.subTest(table=table),contextlib.closing(sqlite3.connect(self.dir/'controller/controller.sqlite')) as c:
    c.execute('INSERT INTO '+table+' VALUES ("effect")');c.commit()
    with self.assertRaisesRegex(ValueError,'Controller records'):self.absence()
    c.execute('DELETE FROM '+table);c.commit()
 def test_live_controller_lock_blocks_reassignment(self):
  with (self.dir/'controller/controller.lock').open('r') as f:
   fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
   with self.assertRaises(BlockingIOError):self.absence()
 def test_actual_worker_identity_or_marker_refuses_setup_recovery(self):
  for key in A.FIELDS:
   d={**self.out,key:'existing'};self.save(self.dir/'result.json',d)
   with self.assertRaises(ValueError):self.absence()
  self.save(self.dir/'result.json',self.out)
  for name in ['operator-admit.json','operator-prepare.json','host-private.json','native-session.json']:
   p=self.dir/name;p.write_text('{}')
   with self.assertRaises(ValueError):self.absence()
   p.unlink()
 def test_permission_denial_and_changed_controller_are_not_context_errors(self):
  self.save(self.dir/'result.json',{**self.out,'error':'Permission denied: external_directory'})
  with self.assertRaises(ValueError):self.absence()
 def test_controller_errors_do_not_count_as_absence(self):
  self.controller.write_text('#!/usr/bin/python3\nimport sys\nsys.stderr.write("configuration changed\\n");raise SystemExit(1)\n');self.save(self.root/'supervisor.json',{'controller':str(self.controller),'controller_sha256':A.sha(self.controller)})
  with self.assertRaisesRegex(ValueError,'authoritatively'):self.absence()
  self.save(self.dir/'result.json',self.out);self.save(self.root/'supervisor.json',{'controller':str(self.controller),'controller_sha256':'0'*64})
  with self.assertRaises(ValueError):self.absence()
 def test_saved_absence_reaudits_result_owner_and_live_ledger(self):
  att={'absence':self.absence(),'owned':A.owner_receipt(self.process,self.mp),'manifest':str(self.mp)};self.save(self.dir/'pre-admission.json',att);self.assertEqual(A.audit(self.root,self.dir,self.controller)['kind'],'not_admitted')
  self.save(self.process,{**json.loads(self.process.read_text()),'quiescent':False})
  with self.assertRaises(ValueError):A.audit(self.root,self.dir,self.controller)
 def recovery_manifest(self):
  (self.root/'repository').mkdir();state=self.root/'dispatch.json';self.state=state
  self.save(self.mp,{'goals':[{'id':'goal','root':str(self.root),'repository':str(self.root/'repository')}],'controller':str(self.controller),'pool_lock':str(self.root/'pool.lock'),'state':str(state),'trusted_files':{str(self.controller):A.sha(self.controller)}})
  self.save(state,{'status':'needs_attention','current':{'goal':'goal','expected_sequence':2},'goals':{'goal':{'state':'paused'}}})
  self.save(self.root/'status.json',{'sequence':2,'status':'paused','current':{'sequence':2},'pause_reason':'Execution/recovery evidence incomplete: '+self.out['error']})
 def test_owner_reconciliation_preserves_failure_and_has_no_native_stop_claim(self):
  self.recovery_manifest();before=(self.dir/'result.json').read_bytes();proof=A.reconcile(self.mp,'goal',self.process)
  self.assertEqual(before,(self.dir/'result.json').read_bytes());self.assertIsNone(json.loads((self.root/'status.json').read_text())['current']);self.assertIsNone(json.loads(self.state.read_text())['current']);self.assertEqual(json.loads(self.state.read_text())['goals']['goal']['native_status'],'not_admitted');self.assertNotIn('execution_stop',proof);self.assertEqual(A.audit(self.root,self.dir,self.controller),proof)
 def test_user_pause_and_live_pool_refuse_reconciliation(self):
  self.recovery_manifest();(self.root/'PAUSE').write_text('user pause')
  with self.assertRaises(ValueError):A.reconcile(self.mp,'goal',self.process)
  self.assertIsNotNone(json.loads((self.root/'status.json').read_text())['current']);(self.root/'PAUSE').unlink()
  with (self.root/'pool.lock').open('a') as lock:
   fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
   with self.assertRaisesRegex(RuntimeError,'Another owner holds'):A.reconcile(self.mp,'goal',self.process)
 def test_incomplete_write_restores_both_owner_records_before_retry(self):
  self.recovery_manifest();old={'status':json.loads((self.root/'status.json').read_text()),'state':json.loads(self.state.read_text())};self.save(self.root/'pre-admission-journal.json',old);self.save(self.root/'status.json',{'status':'waiting','current':None})
  with self.assertRaisesRegex(ValueError,'restored'):A.reconcile(self.mp,'goal',self.process)
  self.assertEqual(json.loads((self.root/'status.json').read_text()),old['status']);self.assertEqual(json.loads(self.state.read_text()),old['state'])
 def test_missing_database_does_not_become_a_new_empty_ledger(self):
  (self.dir/'controller/controller.sqlite').unlink()
  with self.assertRaises(ValueError):self.absence()
 def test_policy_guard_catches_observed_draft_without_running_it(self):
  for source in ["run(['git','init','--bare'])","config='insteadOf = git@github.com:owner/repo'","alias='/tmp/opencode/queue'","Q.Queue.__new__(Q.Queue)"]:
   with self.subTest(source=source),self.assertRaises(ValueError):P.validate_source(source)
  self.assertTrue(P.validate_source("# Never use '--bare' or Q.Queue.__new__\nclass FakeRemote:\n def push(self,b,repo,c):self.head=c\n"))
 def scope_fixture(self):
  self.recovery_manifest();repo=self.root/'repository';git=lambda *args:subprocess.check_output(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@localhost',*args],text=True).strip()
  git('init','-q');(repo/'product.py').write_text('old');git('add','.');git('commit','-qm','initial');base=git('rev-parse','HEAD');(repo/'result.txt').write_text('unaccepted repair');git('add','.');git('commit','-qm','repair input');repair=git('rev-parse','HEAD');git('checkout','--detach',base);(repo/'product.py').write_text('verified upgrade');git('commit','-qam','published implementation');published=git('rev-parse','HEAD');git('checkout','--detach',repair);git('merge','--no-ff',published,'-m','Retain repair input with verified published baseline');merged=git('rev-parse','HEAD')
  cfg=json.loads((self.root/'supervisor.json').read_text());goal=json.loads((self.root/'goal.json').read_text());policy=hashlib.sha256(json.dumps({'cfg':cfg,'goal':goal},separators=(',',':')).encode()).hexdigest();self.save(self.root/'status.json',{'status':'waiting','current':None,'base':repair,'policy_hash':policy});self.save(self.state,{'current':None,'goals':{'goal':{'state':'ready'}}});self.save(self.root/'job-template.json',{'allowed_paths':['result.txt'],'protected_paths':['product.py']})
  pub=self.root/'published.json';self.save(pub,{'binding':{'repository':'lhemerly/code-factory-controller','repository_id':1414818646,'private':True,'branch':'main','required_checks':['rust-and-contract']},'intent':{'state':'confirmed','commit':published,'ci':{'status':'passed'}}});return git,pub,published,merged
 def test_verified_baseline_advance_preserves_only_the_existing_repair_paths(self):
  git,pub,published,merged=self.scope_fixture();result=A.advance_scope(self.mp,'goal',pub,merged)
  self.assertEqual(result['scope_base'],published);self.assertTrue(result['allowed_paths_unchanged']);cfg=json.loads((self.root/'supervisor.json').read_text());self.assertEqual(cfg['scope_base'],published);self.assertEqual(json.loads((self.root/'status.json').read_text())['base'],merged);self.assertEqual(json.loads((self.root/'approved-policy-migration.json').read_text())['kind'],'verified-scope-base-advance')
 def test_baseline_advance_refuses_protected_changes_and_unconfirmed_ci(self):
  git,pub,published,merged=self.scope_fixture();(self.root/'repository/product.py').write_text('unverified protected change');git('commit','-qam','must reject');bad=git('rev-parse','HEAD')
  with self.assertRaisesRegex(ValueError,'widen repair scope'):A.advance_scope(self.mp,'goal',pub,bad)
  d=json.loads(pub.read_text());d['intent']['ci']['status']='pending';self.save(pub,d)
  with self.assertRaisesRegex(ValueError,'verified private-main'):A.advance_scope(self.mp,'goal',pub,merged)
 def test_real_supervisor_context_failure_precedes_job_and_process_creation(self):
  root=self.root/'preflight';repo=root/'repository';repo.mkdir(parents=True);(repo/'file').write_text('baseline')
  for args in [['init','-q'],['add','.'],['-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','baseline']]:subprocess.run(['git','-C',str(repo),*args],check=True)
  base=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip();fake=self.root/'client.mjs';fake.write_text("export const OpenCode={make(){throw Error('native must not be called')}}")
  cfg={'path':'/usr/bin:/bin','native':str(self.controller),'native_sha256':A.sha(self.controller),'controller':str(self.controller),'controller_sha256':A.sha(self.controller),'plugin':str(repo),'plugin_commit':base,'initial_base':base,'ownership_lease_seconds':3600,'ownership_heartbeat_seconds':60,'min_free_bytes':0,'max_state_bytes':100000000,'between_cycles_seconds':0}
  self.save(root/'supervisor.json',cfg);self.save(root/'goal.json',{'id':'preflight','objective':'test'});self.save(root/'protected.json',[]);(root/'task.txt').write_text('oversized instructions '*900)
  p=subprocess.run([shutil.which('node'),str(S/'spark-continuous.mjs'),str(root),'--one-cycle'],env={**os.environ,'FACTORY_OPENCODE_V2_CLIENT':str(fake)},capture_output=True,text=True,timeout=20)
  self.assertEqual(p.returncode,0,p.stderr);status=json.loads((root/'status.json').read_text());out=json.loads((root/'cycles/000001/result.json').read_text());self.assertIsNone(status['current']);self.assertEqual(out['phase'],'context_preflight');self.assertFalse(out['admission_requested']);self.assertNotIn('job_id',out);self.assertFalse(list((root/'cycles/000001').glob('process-*.log')));self.assertEqual(A.context_rejection(root,root/'cycles/000001')['kind'],'not_admitted')

if __name__=='__main__':unittest.main(verbosity=2)
