import copy
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('trace_ledger', ROOT / 'scripts/trace-ledger.py')
m = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(m)
FIXTURE = ROOT / 'tests/fixtures/traces/lifecycle-and-provenance.mock.json'


def sample(run_id='one'):
    return {'format': 'code-factory-trace-summary/v1', 'runs': [{
        'run_id': run_id, 'harness': 'code_factory', 'route': 'opencode',
        'outcome': 'blocked', 'failure_class': 'outcome_unknown'}]}


def normalized(value=None, kind='mock'):
    return m.normalize(json.dumps(value or sample()).encode(), 'sanitized-v1', kind)


class Normalization(unittest.TestCase):
    def test_published_evidence_stays_historical_and_unmeasured(self):
        raw = m.read_reviewed(ROOT / 'docs/live-evidence.json')
        result = m.normalize(raw, 'published-deliveries', 'historical_summary')
        self.assertEqual(len(result['runs']), 2)
        self.assertEqual(result['source_sha256'], m.digest(raw))
        for run in result['runs']:
            self.assertEqual(run['assessment'], 'reported_delivery')
            self.assertEqual(run['evidence_kind'], 'historical_summary')
            self.assertIsNone(run['checks'])
            self.assertIn('unmeasured_checks', run['flags'])
            self.assertIn('lifecycle_unmeasured', run['flags'])
            self.assertEqual(run['authority'], 'observation_only')
            self.assertNotIn('task', run)
            self.assertNotIn('model_tool_calls', run)
        self.assertEqual(result, m.normalize(raw, 'published-deliveries', 'historical_summary'))

    def test_historical_failed_outcome_is_not_lost_as_unknown(self):
        value=json.loads((ROOT/'docs/live-evidence.json').read_text())
        value['runs'][0]['passed']=False
        result=m.normalize(json.dumps(value).encode(),'published-deliveries','historical_summary')
        self.assertEqual(sum(r['reported_outcome']=='failure' for r in result['runs']),1)

    def test_mock_regressions_capture_parent_reported_failure_classes_only(self):
        result = m.normalize(m.read_reviewed(FIXTURE), 'sanitized-v1', 'mock')
        cases = {r['run_id']: r for r in result['runs']}
        process = cases['mock-completion-before-owned-process-exit']
        self.assertEqual(process['assessment'], 'inconsistent_success')
        self.assertIn('owned_processes_running', process['flags'])
        self.assertIn('lifecycle_unresolved', process['flags'])
        init = cases['mock-claimed-transfer-observed-random']
        self.assertEqual(init['assessment'], 'inconsistent_success')
        self.assertIn('initialization_mismatch', init['flags'])
        unknown = cases['mock-blocked-unknown-dispatch']
        self.assertEqual(unknown['assessment'], 'reported_blocked')
        self.assertIn('unresolved_effect', unknown['flags'])
        self.assertTrue(all(r['evidence_kind'] == 'mock' for r in cases.values()))

    def test_unknown_effect_requires_abort_of_same_attempt(self):
        doc = sample()
        run = doc['runs'][0]
        run['outcome'] = 'success'
        run['events'] = [
            {'seq': 1, 'attempt_id': 'first', 'state': 'blocked', 'kind': 'outcome_unknown'},
            {'seq': 2, 'attempt_id': 'second', 'state': 'blocked', 'kind': 'abort'}]
        self.assertEqual(normalized(doc)['runs'][0]['assessment'], 'inconsistent_success')
        run['events'].append({'seq': 3, 'attempt_id': 'first', 'state': 'blocked', 'kind': 'abort'})
        self.assertNotIn('unresolved_effect', normalized(doc)['runs'][0]['flags'])

    def test_delivery_can_be_merged_but_checks_and_delivery_must_bind_integrated_commit(self):
        doc = sample(); run = doc['runs'][0]; run['outcome'] = 'success'
        run['identity'] = {'candidate_commit': 'a'*40, 'integrated_commit': 'b'*40,
                           'delivery_commit': 'b'*40, 'delivery_sha256': 'c'*64}
        run['checks'] = {'executed': 4, 'failed': 0, 'skipped': 0, 'commit': 'b'*40}
        self.assertEqual(normalized(doc)['runs'][0]['assessment'], 'reported_delivery')
        run['checks']['commit'] = 'a'*40
        self.assertIn('check_commit_unbound', normalized(doc)['runs'][0]['flags'])
        run['identity']['delivery_commit'] = 'a'*40
        self.assertIn('delivery_commit_mismatch', normalized(doc)['runs'][0]['flags'])

    def test_failed_zero_skipped_checks_never_count_as_clean_success(self):
        for checks in [{'executed': 0, 'failed': 0, 'skipped': 0},
                       {'executed': 1, 'failed': 1, 'skipped': 0},
                       {'executed': 1, 'failed': 0, 'skipped': 1}]:
            doc = sample(); doc['runs'][0].update(outcome='success', checks=checks)
            self.assertEqual(normalized(doc)['runs'][0]['assessment'], 'inconsistent_success')

    def test_initialization_claims_need_consistency_and_metadata_binding(self):
        doc = sample(); run = doc['runs'][0]; run['outcome'] = 'success'
        run['initialization'] = {'declared': 'transferred', 'observed': 'transferred'}
        self.assertIn('initialization_evidence_unbound', normalized(doc)['runs'][0]['flags'])
        run['initialization']['evidence_sha256'] = 'a'*64
        self.assertNotIn('initialization_evidence_unbound', normalized(doc)['runs'][0]['flags'])
        # Presence of a digest still does not verify its referenced file or create authority.
        self.assertEqual(normalized(doc)['runs'][0]['authority'], 'observation_only')

    def test_unknown_initialization_is_not_fabricated_as_a_measured_mismatch(self):
        doc=sample();doc['runs'][0]['initialization']={'declared':'transferred','observed':'unknown'}
        flags=normalized(doc)['runs'][0]['flags']
        self.assertIn('initialization_unmeasured',flags)
        self.assertNotIn('initialization_mismatch',flags)

    def test_rejects_commands_credentials_authorization_and_other_factory_routes(self):
        for field in ['prompt', 'messages', 'authorization', 'command', 'api_key', 'allowed_paths']:
            doc = sample(); doc['runs'][0][field] = 'must-not-be-stored'
            with self.assertRaises(ValueError): normalized(doc)
        doc = sample(); doc['runs'][0]['route'] = 'llama_cpp'
        with self.assertRaises(ValueError): normalized(doc)

    def test_strict_json_bounds_and_types(self):
        for raw in [b'{"format":"a","format":"b","runs":[]}', b'{"x":NaN}', b'x'*(m.MAX_BYTES+1)]:
            with self.assertRaises(ValueError): m.normalize(raw, 'sanitized-v1', 'mock')
        for bad in [True, -1, 0.5, 2**60]:
            doc = sample(); doc['runs'][0]['checks'] = {'executed': bad, 'failed': 0, 'skipped': 0}
            with self.assertRaises(ValueError): normalized(doc)
        for bad in [-1, True, float('inf'), 10**400]:
            doc = sample(); doc['runs'][0]['reported_cost'] = bad
            with self.assertRaises(ValueError): normalized(doc)
        doc = sample(); doc['runs'] *= m.MAX_RUNS+1
        with self.assertRaises(ValueError): normalized(doc)
        doc = sample(); doc['runs'][0]['events'] = [dict(seq=1, attempt_id='a', state='running', kind='observation')]*2
        with self.assertRaises(ValueError): normalized(doc)

    def test_input_symlink_fifo_and_oversize_are_not_read(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d); (root/'file').write_bytes(b'{}'); (root/'link').symlink_to(root/'file')
            with self.assertRaises(OSError): m.read_reviewed(root/'link')
            os.mkfifo(root/'fifo')
            with self.assertRaises(ValueError): m.read_reviewed(root/'fifo')
            (root/'large').write_bytes(b'x'*(m.MAX_BYTES+1))
            with self.assertRaises(ValueError): m.read_reviewed(root/'large')


class Persistence(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.path = Path(self.temp.name)/'ledger.sqlite'
        self.ledger = m.Ledger(self.path)

    def tearDown(self):
        self.ledger.close(); self.temp.cleanup()

    def test_import_restart_idempotency_and_readonly_replay(self):
        doc = normalized()
        self.assertEqual(self.ledger.import_summary('source', doc)['status'], 'imported')
        self.ledger.close(); self.ledger = m.Ledger(self.path)
        self.assertEqual(self.ledger.import_summary('source', doc)['status'], 'already_imported')
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0], 1)
        reader = m.Ledger(self.path, read_only=True)
        try:
            self.assertTrue(reader.replay('source', doc)['passed'])
            with self.assertRaises(sqlite3.OperationalError): reader.db.execute('DELETE FROM trace_runs')
        finally: reader.close()
        self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)

    def test_conflicts_relabeling_and_duplicate_runs_rollback(self):
        doc = normalized(); self.ledger.import_summary('source', doc)
        with self.assertRaises(ValueError): self.ledger.import_summary('renamed', doc)
        changed = normalized(sample('changed'))
        with self.assertRaises(ValueError): self.ledger.import_summary('source', changed)
        relabeled = normalized(kind='sanitized_experiment')
        with self.assertRaises(ValueError): self.ledger.import_summary('source', relabeled)
        doc2 = sample('new'); doc2['runs'].append(sample()['runs'][0])
        with self.assertRaises(sqlite3.IntegrityError): self.ledger.import_summary('collision', normalized(doc2))
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_sources').fetchone()[0], 1)
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0], 1)

    def test_concurrent_duplicate_imports_have_one_effect(self):
        source=Path(self.temp.name)/'concurrent.json';source.write_text(json.dumps(sample()))
        command=[sys.executable,str(ROOT/'scripts/trace-ledger.py'),'import','--db',str(self.path),
                 '--input',str(source),'--source-id','concurrent','--format','sanitized-v1',
                 '--evidence-kind','mock','--reviewed-sanitized']
        first=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        second=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        outputs=[process.communicate(timeout=10) for process in (first,second)]
        self.assertEqual([first.returncode,second.returncode],[0,0],outputs)
        self.assertEqual(sorted(json.loads(output[0])['status'] for output in outputs),['already_imported','imported'])
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0],1)

    def test_injected_mid_batch_failure_is_atomic_and_retryable(self):
        self.ledger.db.execute("CREATE TRIGGER fail_second BEFORE INSERT ON trace_runs WHEN instr(NEW.id,'two')>0 BEGIN SELECT RAISE(ABORT,'test fault'); END")
        doc = sample(); doc['runs'].append(sample('two')['runs'][0]); doc = normalized(doc)
        with self.assertRaises(sqlite3.IntegrityError): self.ledger.import_summary('batch', doc)
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_sources').fetchone()[0], 0)
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0], 0)
        self.ledger.db.execute('DROP TRIGGER fail_second')
        self.assertEqual(self.ledger.import_summary('batch', doc)['status'], 'imported')

    def test_process_death_rolls_back_uncommitted_import(self):
        self.ledger.import_summary('stable', normalized())
        script = "import sqlite3,os,signal,sys; d=sqlite3.connect(sys.argv[1]); d.execute('BEGIN IMMEDIATE'); d.execute(\"INSERT INTO trace_sources VALUES('interrupted','dead','{}')\"); os.kill(os.getpid(),signal.SIGKILL)"
        result = subprocess.run([sys.executable, '-c', script, str(self.path)], capture_output=True, timeout=5)
        self.assertLess(result.returncode, 0)
        self.ledger.close(); self.ledger = m.Ledger(self.path)
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_sources').fetchone()[0], 1)
        self.assertEqual(self.ledger.import_summary('stable', normalized())['status'], 'already_imported')

    def test_replay_detects_changed_bytes_semantics_and_corruption(self):
        doc = normalized(); self.ledger.import_summary('source', doc)
        changed = copy.deepcopy(doc); changed['runs'][0]['assessment'] = 'reported_success'
        with self.assertRaises(ValueError): self.ledger.replay('source', changed)
        changed = copy.deepcopy(doc); changed['source_sha256'] = 'b'*64
        with self.assertRaises(ValueError): self.ledger.replay('source', changed)
        self.ledger.db.execute("UPDATE trace_runs SET normalized='{}'"); self.ledger.db.commit()
        with self.assertRaises(ValueError): self.ledger.replay('source', doc)
        with self.assertRaises(ValueError): self.ledger.report()

    def test_accounting_separates_harness_evidence_and_missing_measurements(self):
        raw = m.read_reviewed(ROOT/'docs/live-evidence.json')
        self.ledger.import_summary('history', m.normalize(raw, 'published-deliveries', 'historical_summary'))
        self.ledger.import_summary('mock', m.normalize(m.read_reviewed(FIXTURE), 'sanitized-v1', 'mock'))
        report = self.ledger.report()
        groups = {(g['harness'], g['evidence_kind']): g for g in report['groups']}
        history = groups['code_factory', 'historical_summary']
        self.assertEqual(history['runs'], 2); self.assertEqual(history['checks_measured_runs'], 0)
        self.assertIsNone(history['executed'])
        self.assertEqual(history['cost_known_runs'], 2); self.assertEqual(history['reported_cost_sum'], 0)
        research = groups['research', 'mock']
        self.assertEqual(research['assessments']['inconsistent_success'], 2)
        self.assertEqual(research['cost_known_runs'], 0)
        self.assertIsNone(research['reported_cost_sum'])
        self.assertEqual(report['readiness'], 'not_assessed')
        self.assertFalse(report['live_model_qualification'])
        self.assertEqual(report['authority'], 'observation_only')

    def test_capacity_refuses_batch_before_any_import_effect(self):
        with patch.object(m, 'MAX_TOTAL_RUNS', 0):
            with self.assertRaises(ValueError): self.ledger.import_summary('source', normalized())
        self.assertEqual(self.ledger.db.execute('SELECT COUNT(*) FROM trace_sources').fetchone()[0], 0)

    def test_controller_database_and_ledger_version_are_not_migrated(self):
        other = Path(self.temp.name)/'controller.sqlite'
        with sqlite3.connect(other) as db: db.execute('CREATE TABLE jobs(id TEXT)')
        before = other.read_bytes()
        with self.assertRaises(ValueError): m.Ledger(other)
        self.assertEqual(before, other.read_bytes())
        self.ledger.db.execute('UPDATE trace_meta SET version=99'); self.ledger.db.commit()
        with self.assertRaises(ValueError): m.Ledger(self.path)

    def test_cli_requires_review_and_does_not_echo_rejected_private_content(self):
        source = Path(self.temp.name)/'source.json'; source.write_text(json.dumps(sample()))
        command = [sys.executable, str(ROOT/'scripts/trace-ledger.py'), 'import', '--db', str(self.path),
                   '--input', str(source), '--source-id', 'cli', '--format', 'sanitized-v1', '--evidence-kind', 'mock']
        rejected = subprocess.run(command, capture_output=True, text=True, timeout=5)
        self.assertEqual(rejected.returncode, 1)
        source.write_text(json.dumps({'token': 'DO-NOT-ECHO-PRIVATE'}))
        rejected = subprocess.run(command+['--reviewed-sanitized'], capture_output=True, text=True, timeout=5)
        self.assertEqual(rejected.returncode, 1); self.assertNotIn('DO-NOT-ECHO-PRIVATE', rejected.stdout+rejected.stderr)
        source.write_text(json.dumps(sample()))
        accepted = subprocess.run(command+['--reviewed-sanitized'], capture_output=True, text=True, timeout=5)
        self.assertEqual(accepted.returncode, 0, accepted.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
