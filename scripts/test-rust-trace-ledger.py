"""Bidirectional persisted-ledger parity using frozen sanitized corpora."""
import os
import pathlib,sys,importlib.util,json,subprocess,tempfile,unittest,sqlite3,hashlib
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve();sp=importlib.util.spec_from_file_location('frozen_trace',BASE/'scripts/trace-ledger.py');legacy=importlib.util.module_from_spec(sp);sp.loader.exec_module(legacy)
CORPORA=[('history',BASE/'docs/live-evidence.json','published-deliveries','historical_summary'),('mock',BASE/'tests/fixtures/traces/lifecycle-and-provenance.mock.json','sanitized-v1','mock')]
class NativeLedgerParity(unittest.TestCase):
 def invoke(self,action,db,case=None):
  argv=[str(native),'trace-ledger',action,'--db',str(db)]
  if case:
   name,source,fmt,kind=case;argv+=['--input',str(source),'--source-id',name,'--format',fmt,'--evidence-kind',kind,'--reviewed-sanitized']
  return subprocess.run(argv,capture_output=True,text=True,timeout=15)
 def test_bidirectional_persisted_corpora(self):
  for case in CORPORA:
   with self.subTest(case=case[0]),tempfile.TemporaryDirectory() as tmp:
    root=pathlib.Path(tmp);name,source,fmt,kind=case;normalized=legacy.normalize(source.read_bytes(),fmt,kind);pdb=root/'python.db';ndb=root/'rust.db';ledger=legacy.Ledger(pdb)
    try:ledger.import_summary(name,normalized)
    finally:ledger.close()
    r=self.invoke('replay',pdb,case);self.assertEqual(r.returncode,0,r.stdout+r.stderr);self.assertTrue(json.loads(r.stdout)['passed']);r=self.invoke('import',ndb,case);self.assertEqual(r.returncode,0,r.stdout+r.stderr)
    ledger=legacy.Ledger(ndb,read_only=True)
    try:self.assertTrue(ledger.replay(name,normalized)['passed']);expected=ledger.report()
    finally:ledger.close()
    r=self.invoke('report',ndb);self.assertEqual(r.returncode,0,r.stdout+r.stderr);self.assertEqual(json.loads(r.stdout),expected)
 def test_ambiguous_json_rejected_without_database_creation(self):
  for payload in [b'{"format":"x","format":"y"}',b'{"cost":NaN}',b'{} trailing']:
   with self.subTest(payload=payload),tempfile.TemporaryDirectory() as tmp:
    root=pathlib.Path(tmp);source=root/'input.json';source.write_bytes(payload);db=root/'new.db';case=('invalid',source,'sanitized-v1','mock');r=self.invoke('import',db,case);self.assertNotEqual(r.returncode,0);self.assertFalse(db.exists())
 def test_unrelated_database_is_unchanged(self):
  with tempfile.TemporaryDirectory() as tmp:
   db=pathlib.Path(tmp)/'controller.db';c=sqlite3.connect(db);c.execute('CREATE TABLE jobs(id TEXT)');c.commit();c.close();before=db.read_bytes();r=self.invoke('import',db,CORPORA[1]);self.assertNotEqual(r.returncode,0);self.assertEqual(db.read_bytes(),before)
 def test_multiple_schema_versions_rejected(self):
  with tempfile.TemporaryDirectory() as tmp:
   db=pathlib.Path(tmp)/'bad.db';r=self.invoke('import',db,CORPORA[1]);self.assertEqual(r.returncode,0,r.stdout+r.stderr);c=sqlite3.connect(db);c.execute('INSERT INTO trace_meta VALUES(1)');c.commit();c.close()
   with self.assertRaises(ValueError):legacy.Ledger(db,read_only=True)
   r=self.invoke('report',db);self.assertNotEqual(r.returncode,0)
suite=unittest.defaultTestLoader.loadTestsFromTestCase(NativeLedgerParity);r=unittest.TextTestRunner(verbosity=2).run(suite);raise SystemExit(not r.wasSuccessful())
