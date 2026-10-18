"""Native SQLite contention and transactional rollback, local synthetic data."""
import os
import pathlib,sys,subprocess,json,importlib.util,tempfile,sqlite3,time,unittest
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve();sp=importlib.util.spec_from_file_location('frozen_trace_contention',BASE/'scripts/trace-ledger.py');legacy=importlib.util.module_from_spec(sp);sp.loader.exec_module(legacy)
def argv(db,source):return [str(native),'trace-ledger','import','--db',str(db),'--input',str(source),'--source-id','source','--format','sanitized-v1','--evidence-kind','mock','--reviewed-sanitized']
def sample(ids):return {'format':'code-factory-trace-summary/v1','runs':[{'run_id':name,'harness':'code_factory','route':'opencode','outcome':'blocked','failure_class':'outcome_unknown'} for name in ids]}
class NativeContention(unittest.TestCase):
 def test_existing_ledger_concurrent_import_waits_and_has_one_effect(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp);db=root/'ledger.db';source=root/'source.json';source.write_text(json.dumps(sample(['one'])));ledger=legacy.Ledger(db);ledger.close();hold=sqlite3.connect(db);hold.execute('BEGIN IMMEDIATE');first=subprocess.Popen(argv(db,source),stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True);second=subprocess.Popen(argv(db,source),stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
   try:
    time.sleep(.1);hold.rollback();hold.close();outputs=[p.communicate(timeout=15) for p in [first,second]];self.assertEqual([first.returncode,second.returncode],[0,0],outputs);self.assertEqual(sorted(json.loads(out[0])['status'] for out in outputs),['already_imported','imported'])
    with sqlite3.connect(db) as c:self.assertEqual(c.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0],1)
   finally:
    for p in [first,second]:
     if p.poll() is None:p.kill();p.communicate(timeout=5)
 def test_injected_mid_batch_failure_rolls_back_and_retry_has_one_effect(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp);db=root/'ledger.db';source=root/'source.json';source.write_text(json.dumps(sample(['one','two'])));ledger=legacy.Ledger(db);ledger.close()
   with sqlite3.connect(db) as c:c.execute("CREATE TRIGGER fail_second BEFORE INSERT ON trace_runs WHEN instr(NEW.id,'two')>0 BEGIN SELECT RAISE(ABORT,'test fault'); END")
   p=subprocess.run(argv(db,source),capture_output=True,text=True,timeout=15);self.assertNotEqual(p.returncode,0)
   with sqlite3.connect(db) as c:
    self.assertEqual(c.execute('SELECT COUNT(*) FROM trace_sources').fetchone()[0],0);self.assertEqual(c.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0],0);c.execute('DROP TRIGGER fail_second')
   p=subprocess.run(argv(db,source),capture_output=True,text=True,timeout=15);self.assertEqual(p.returncode,0,p.stdout+p.stderr)
   with sqlite3.connect(db) as c:self.assertEqual(c.execute('SELECT COUNT(*) FROM trace_sources').fetchone()[0],1);self.assertEqual(c.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0],2)
r=unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(NativeContention));raise SystemExit(not r.wasSuccessful())
