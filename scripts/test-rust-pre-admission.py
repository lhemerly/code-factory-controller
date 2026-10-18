"""Independent native CLI controls built on frozen pre-admission fixtures."""
import os
import pathlib,sys,importlib.util,json,subprocess,unittest,fcntl,sqlite3
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve();sp=importlib.util.spec_from_file_location('frozen_native_pre_admission_cli',BASE/'scripts/test-pre-admission-recovery.py');legacy=importlib.util.module_from_spec(sp);sp.loader.exec_module(legacy)
class NativePreAdmissionCli(legacy.PreAdmissionRegression):
 def invoke(self,action):
  args=['--root',str(self.root),'--directory',str(self.dir),'--controller',str(self.controller)] if action=='audit' else ['--manifest',str(self.mp),'--goal','goal','--owned-receipt',str(self.process)]
  return subprocess.run([str(native),'pre-admission-recovery',action,*args],capture_output=True,text=True,timeout=30)
 def test_cli_saved_absence_reaudits_receipt_and_preserves_failure(self):
  proof=self.absence();att={'absence':proof,'owned':legacy.A.owner_receipt(self.process,self.mp),'manifest':str(self.mp)};self.save(self.dir/'pre-admission.json',att);before=(self.dir/'result.json').read_bytes();p=self.invoke('audit');self.assertEqual(p.returncode,0,p.stderr);self.assertEqual(json.loads(p.stdout),proof);self.save(self.process,{**json.loads(self.process.read_text()),'quiescent':False});p=self.invoke('audit');self.assertNotEqual(p.returncode,0);self.assertEqual((self.dir/'result.json').read_bytes(),before)
 def test_cli_reconcile_outputs_a_python_auditable_absence(self):
  self.recovery_manifest();before=(self.dir/'result.json').read_bytes();p=self.invoke('reconcile');self.assertEqual(p.returncode,0,p.stderr);proof=json.loads(p.stdout);self.assertEqual(legacy.A.audit(self.root,self.dir,self.controller),proof);self.assertEqual((self.dir/'result.json').read_bytes(),before);self.assertNotIn('execution_stop',proof);self.assertNotIn('accounting',proof);status=json.loads((self.root/'status.json').read_text());state=json.loads(self.state.read_text());self.assertIsNone(status['current']);self.assertIsNone(state['current']);self.assertIsNone(state['goals']['goal']['usage']);self.assertIsNone(state['goals']['goal']['model']);self.assertEqual(state['goals']['goal']['native_status'],'not_admitted')
 def test_cli_user_pause_and_live_pool_lock_fence_mutation(self):
  self.recovery_manifest();before={p:p.read_bytes() for p in [self.root/'status.json',self.state,self.dir/'result.json']};pause=self.root/'PAUSE';pause.write_text('human pause');p=self.invoke('reconcile');self.assertNotEqual(p.returncode,0);self.assertEqual(pause.read_text(),'human pause');pause.unlink()
  with (self.root/'pool.lock').open('a') as lock:
   fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB);p=self.invoke('reconcile');self.assertNotEqual(p.returncode,0)
  for path,data in before.items():self.assertEqual(path.read_bytes(),data)
 def test_cli_interrupted_native_journal_restores_exact_bytes(self):
  self.recovery_manifest();status_path=self.root/'status.json';status=status_path.read_text();state=self.state.read_text();before={'status':json.loads(status),'state':json.loads(state),'raw_files':{'status':status,'state':state}};self.save(self.root/'pre-admission-journal.json',before);status_path.write_text('{"status":"waiting","current":null}');self.state.write_text('{"current":null}');p=self.invoke('reconcile');self.assertNotEqual(p.returncode,0);self.assertIn('restored',p.stderr);self.assertEqual(status_path.read_text(),status);self.assertEqual(self.state.read_text(),state);self.assertFalse((self.root/'pre-admission-journal.json').exists())
 def test_cli_nonempty_ledger_cannot_be_reassigned(self):
  self.recovery_manifest();before={p:p.read_bytes() for p in [self.root/'status.json',self.state,self.dir/'result.json']}
  with sqlite3.connect(self.dir/'controller/controller.sqlite') as db:db.execute('INSERT INTO jobs VALUES ("effect")')
  p=self.invoke('reconcile');self.assertNotEqual(p.returncode,0);self.assertIn('Controller records',p.stderr)
  for path,data in before.items():self.assertEqual(path.read_bytes(),data)
names=[name for name in vars(NativePreAdmissionCli) if name.startswith('test_cli_')];r=unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(NativePreAdmissionCli(name) for name in names));raise SystemExit(not r.wasSuccessful())
