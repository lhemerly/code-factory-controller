#!/usr/bin/env python3
"""Offline, non-authoritative ledger for explicitly reviewed sanitized summaries."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import stat

VERSION = 1
MAX_BYTES = 1_048_576
MAX_RUNS = 256
MAX_EVENTS = 256
MAX_TOTAL_RUNS = 10_000
KINDS = {'historical_summary', 'sanitized_experiment', 'mock'}
STATES = {'queued', 'running', 'verifying', 'retry_wait', 'paused', 'blocked', 'done'}
EVENTS = {'observation', 'dispatch_intent', 'dispatch_ack', 'outcome_unknown',
          'abort', 'verification_pass', 'verification_fail', 'delivered'}
FAILURES = {'none', 'provider', 'timeout', 'outcome_unknown', 'verification',
            'scope', 'budget', 'cancelled', 'unknown'}
TABLES = {'trace_meta', 'trace_sources', 'trace_runs'}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False)


def digest(value):
    return hashlib.sha256(value).hexdigest()


def shape(value, allowed, required=()):
    if not isinstance(value, dict) or set(value) - set(allowed) or not set(required) <= set(value):
        raise ValueError('unsupported fields or missing required fields in sanitized summary')


def ident(value):
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:/-]{0,159}', value):
        raise ValueError('invalid bounded summary identifier')
    return value


def integer(value):
    if type(value) is not int or not 0 <= value <= 2**53 - 1:
        raise ValueError('invalid nonnegative integer counter')
    return value


def hexdigest(value, lengths=(40, 64)):
    if not isinstance(value, str) or len(value) not in lengths or not re.fullmatch('[0-9a-f]+', value):
        raise ValueError('invalid commit or digest')
    return value


def cost(value):
    if value is not None and (type(value) not in (int, float) or value < 0 or value > 1_000_000 or not math.isfinite(value)):
        raise ValueError('invalid reported cost')
    return value


def read_reviewed(path):
    # No discovery, recursive reads, symlinks, session-directory traversal or auth.
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            raise ValueError('input must be an explicitly selected regular file')
        raw = source.read(MAX_BYTES + 1)
    if len(raw) > MAX_BYTES:
        raise ValueError('summary exceeds byte limit')
    return raw


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError('duplicate JSON object key')
        value[key] = item
    return value


def load(raw):
    if len(raw) > MAX_BYTES:
        raise ValueError('summary exceeds byte limit')
    return json.loads(raw, object_pairs_hook=unique_object,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite JSON')))


def historical(document):
    """Adapt only the repository's published summary schema; never raw sessions."""
    shape(document, {'date', 'task', 'runs'}, {'date', 'runs'})
    if not isinstance(document['date'], str) or not re.fullmatch(r'\d{4}-\d{2}-\d{2}', document['date']):
        raise ValueError('invalid historical date')
    runs = document['runs']
    if not isinstance(runs, list) or not 1 <= len(runs) <= MAX_RUNS:
        raise ValueError('invalid bounded run list')
    result = []
    for run in runs:
        shape(run, {'passed', 'mode', 'controller_version', 'opencode_version', 'model',
                    'source_repository', 'base_commit', 'candidate_commit', 'integrated_commit',
                    'delivery', 'reported_cost', 'final_state', 'final_outcome',
                    'restart_reconciled', 'model_tool_calls'},
              {'passed', 'mode', 'source_repository', 'base_commit', 'candidate_commit',
               'integrated_commit', 'delivery', 'final_state', 'final_outcome'})
        if type(run['passed']) is not bool or run['mode'] not in {'adapter', 'plugin'}:
            raise ValueError('invalid historical outcome or route')
        delivery = run['delivery']
        shape(delivery, {'reference', 'sha256', 'commit'}, {'sha256', 'commit'})
        # Stable across source relabeling/reformatting; no session ID was published.
        key = [run['source_repository'], run['base_commit'], run['candidate_commit'], run['mode']]
        converted = {'run_id': 'historical-' + digest(canonical(key).encode())[:32],
                     'harness': 'code_factory', 'route': 'opencode',
                     'outcome': ('failure' if not run['passed'] else 'success'
                                 if run['final_state'] == 'done' and run['final_outcome'] == 'verified_delivery'
                                 else 'blocked' if run['final_state'] in {'blocked', 'paused'} else 'incomplete'),
                     'identity': {key: run[key] for key in ('candidate_commit', 'integrated_commit')}}
        converted['identity'].update(delivery_commit=delivery['commit'], delivery_sha256=delivery['sha256'])
        converted['reported_cost'] = run.get('reported_cost')
        if 'restart_reconciled' in run:
            converted['restart_reconciled'] = run['restart_reconciled']
        result.append(converted)
    return result


def normalize(raw, source_format, evidence_kind):
    if evidence_kind not in KINDS:
        raise ValueError('unsupported evidence kind')
    document = load(raw)
    if source_format == 'published-deliveries':
        if evidence_kind != 'historical_summary':
            raise ValueError('published summaries must retain historical evidence kind')
        runs = historical(document)
    elif source_format == 'sanitized-v1':
        shape(document, {'format', 'runs'}, {'format', 'runs'})
        if document['format'] != 'code-factory-trace-summary/v1':
            raise ValueError('unsupported sanitized summary version')
        runs = document['runs']
    else:
        raise ValueError('unsupported summary format')
    if not isinstance(runs, list) or not 1 <= len(runs) <= MAX_RUNS:
        raise ValueError('invalid bounded run list')
    normalized, keys = [], set()
    for run in runs:
        shape(run, {'run_id', 'harness', 'route', 'outcome', 'failure_class', 'job_id',
                    'identity', 'checks', 'events', 'reported_cost', 'restart_reconciled',
                    'lifecycle', 'initialization'},
              {'run_id', 'harness', 'route', 'outcome'})
        if run['harness'] not in {'code_factory', 'research'} or run['route'] not in {'opencode', 'not_applicable'}:
            raise ValueError('unsupported harness or execution route')
        if run['harness'] == 'code_factory' and run['route'] != 'opencode':
            raise ValueError('this trace increment supports only OpenCode factory execution')
        if run['outcome'] not in {'success', 'failure', 'blocked', 'incomplete', 'unknown'}:
            raise ValueError('unsupported reported outcome')
        failure = run.get('failure_class', 'unknown')
        if failure not in FAILURES:
            raise ValueError('unsupported bounded failure class')
        item = {'run_id': ident(run['run_id']), 'harness': run['harness'], 'route': run['route'],
                'evidence_kind': evidence_kind, 'reported_outcome': run['outcome'],
                'failure_class': failure, 'reported_cost': cost(run.get('reported_cost')),
                'checks': None, 'identity': {}, 'events': [], 'flags': [],
                'authority': 'observation_only'}
        key = canonical([item['harness'], item['route'], item['run_id']])
        if key in keys:
            raise ValueError('duplicate logical run in input')
        keys.add(key)
        if 'job_id' in run:
            item['job_id'] = ident(run['job_id'])
        if 'restart_reconciled' in run:
            if type(run['restart_reconciled']) is not bool:
                raise ValueError('restart result must be an explicit boolean')
            item['restart_reconciled'] = run['restart_reconciled']
        if 'lifecycle' in run:
            lifecycle = run['lifecycle']
            shape(lifecycle, {'owned_processes_running', 'owned_processes_unknown', 'cleanup'},
                  {'owned_processes_running', 'owned_processes_unknown', 'cleanup'})
            item['lifecycle'] = {key: integer(lifecycle[key]) for key in
                                 ('owned_processes_running', 'owned_processes_unknown')}
            if lifecycle['cleanup'] not in {'confirmed', 'pending', 'unknown', 'not_applicable'}:
                raise ValueError('unsupported lifecycle cleanup observation')
            item['lifecycle']['cleanup'] = lifecycle['cleanup']
            if lifecycle['owned_processes_running']:
                item['flags'].append('owned_processes_running')
            if lifecycle['owned_processes_unknown'] or lifecycle['cleanup'] in {'pending', 'unknown'}:
                item['flags'].append('lifecycle_unresolved')
        else:
            item['flags'].append('lifecycle_unmeasured')
        if 'initialization' in run:
            initialization = run['initialization']
            shape(initialization, {'declared', 'observed', 'evidence_sha256'}, {'declared', 'observed'})
            methods = {'random', 'transferred', 'resumed', 'not_applicable', 'unknown'}
            if initialization['declared'] not in methods or initialization['observed'] not in methods:
                raise ValueError('unsupported initialization observation')
            item['initialization'] = dict(initialization)
            if 'evidence_sha256' in initialization:
                hexdigest(initialization['evidence_sha256'], (64,))
            if 'unknown' in (initialization['declared'], initialization['observed']):
                item['flags'].append('initialization_unmeasured')
            elif initialization['declared'] != initialization['observed']:
                item['flags'].append('initialization_mismatch')
            elif initialization['observed'] in {'transferred', 'resumed'} and 'evidence_sha256' not in initialization:
                item['flags'].append('initialization_evidence_unbound')
        else:
            item['flags'].append('initialization_unmeasured')
        identity = run.get('identity', {})
        shape(identity, {'candidate_commit', 'integrated_commit', 'delivery_commit', 'delivery_sha256'})
        for name, value in identity.items():
            item['identity'][name] = hexdigest(value, (64,) if name == 'delivery_sha256' else (40, 64))
        if 'checks' in run:
            checks = run['checks']
            shape(checks, {'executed', 'failed', 'skipped', 'commit'}, {'executed', 'failed', 'skipped'})
            item['checks'] = {key: integer(checks[key]) for key in ('executed', 'failed', 'skipped')}
            if checks['failed'] > checks['executed']:
                raise ValueError('failed count exceeds execution count')
            if 'commit' in checks:
                item['checks']['commit'] = hexdigest(checks['commit'])
        events = run.get('events', [])
        if not isinstance(events, list) or len(events) > MAX_EVENTS:
            raise ValueError('event limit exceeded')
        previous, unresolved = -1, set()
        for event in events:
            shape(event, {'seq', 'attempt_id', 'state', 'kind'}, {'seq', 'attempt_id', 'state', 'kind'})
            seq = integer(event['seq'])
            if seq <= previous or event['state'] not in STATES or event['kind'] not in EVENTS:
                raise ValueError('unordered or unsupported event observation')
            attempt = ident(event['attempt_id'])
            previous = seq
            item['events'].append(dict(event))
            if event['kind'] == 'outcome_unknown':
                unresolved.add(attempt)
            elif event['kind'] == 'abort':
                unresolved.discard(attempt)
        if unresolved:
            item['flags'].append('unresolved_effect')
        identity = item['identity']
        delivery_claim = all(k in identity for k in ('candidate_commit', 'integrated_commit', 'delivery_commit', 'delivery_sha256'))
        if delivery_claim and identity['integrated_commit'] != identity['delivery_commit']:
            item['flags'].append('delivery_commit_mismatch')
        checks = item['checks']
        if checks is None:
            item['flags'].append('unmeasured_checks')
        elif checks['executed'] == 0 or checks['failed'] or checks['skipped']:
            item['flags'].append('checks_not_clean')
        if checks is not None and delivery_claim and checks.get('commit') != identity['integrated_commit']:
            item['flags'].append('check_commit_unbound')
        inconsistent = run['outcome'] == 'success' and any(flag not in {'unmeasured_checks', 'lifecycle_unmeasured', 'initialization_unmeasured'} for flag in item['flags'])
        item['assessment'] = ('inconsistent_success' if inconsistent else
                              'reported_delivery' if delivery_claim and run['outcome'] == 'success' else
                              'reported_' + run['outcome'])
        item['flags'].sort()
        normalized.append(item)
    return {'normalizer_version': VERSION, 'source_sha256': digest(raw), 'source_format': source_format,
            'evidence_kind': evidence_kind, 'runs': sorted(normalized, key=lambda r: (r['harness'], r['route'], r['run_id']))}


class Ledger:
    def __init__(self, path, read_only=False):
        path = Path(path)
        if path.is_symlink():
            raise ValueError('ledger symlinks are not supported')
        uri = path.resolve().as_uri() + '?mode=ro'
        if path.exists():
            if not path.is_file():
                raise ValueError('ledger must be a regular file')
            with sqlite3.connect(uri, uri=True, timeout=5) as probe:
                tables = {row[0] for row in probe.execute("SELECT name FROM sqlite_master WHERE type='table'")}
                if tables and tables != TABLES:
                    raise ValueError('refusing a controller or unrelated database')
            probe.close()
        else:
            if read_only:
                raise ValueError('ledger does not exist')
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
            os.close(fd)
        self.db = sqlite3.connect(uri, uri=True, timeout=5) if read_only else sqlite3.connect(path, timeout=5)
        tables = {row[0] for row in self.db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if tables and tables != TABLES:
            self.db.close()
            raise ValueError('refusing a controller or unrelated database')
        if not tables:
            if read_only:
                self.db.close()
                raise ValueError('not a trace ledger')
            self.db.executescript('''
                CREATE TABLE trace_meta(version INTEGER NOT NULL);
                INSERT INTO trace_meta VALUES(1);
                CREATE TABLE trace_sources(id TEXT PRIMARY KEY, sha TEXT NOT NULL UNIQUE, policy TEXT NOT NULL);
                CREATE TABLE trace_runs(id TEXT PRIMARY KEY, source TEXT NOT NULL REFERENCES trace_sources(id), normalized TEXT NOT NULL, sha TEXT NOT NULL);
            ''')
        if self.db.execute('SELECT version FROM trace_meta').fetchall() != [(VERSION,)]:
            self.db.close()
            raise ValueError('unsupported ledger version; explicit migration required')
        self.db.execute('PRAGMA foreign_keys=ON')
        if not read_only:
            self.db.execute('PRAGMA journal_mode=WAL')
            self.db.execute('PRAGMA synchronous=FULL')

    def close(self):
        self.db.close()

    def import_summary(self, source_id, document):
        ident(source_id)
        policy = canonical({key: document[key] for key in ('normalizer_version', 'source_format', 'evidence_kind')})
        with self.db:
            self.db.execute('BEGIN IMMEDIATE')
            known = self.db.execute('SELECT sha,policy FROM trace_sources WHERE id=?', (source_id,)).fetchone()
            if known:
                if known != (document['source_sha256'], policy):
                    raise ValueError('source identity conflict; original provenance is immutable')
                self.replay(source_id, document)
                return {'status': 'already_imported', 'source_id': source_id, 'runs': len(document['runs'])}
            if self.db.execute('SELECT 1 FROM trace_sources WHERE sha=?', (document['source_sha256'],)).fetchone():
                raise ValueError('source bytes already imported under another identity')
            count = self.db.execute('SELECT COUNT(*) FROM trace_runs').fetchone()[0]
            if count + len(document['runs']) > MAX_TOTAL_RUNS:
                raise ValueError('ledger run capacity exceeded')
            self.db.execute('INSERT INTO trace_sources VALUES(?,?,?)', (source_id, document['source_sha256'], policy))
            for run in document['runs']:
                key = canonical([run['harness'], run['route'], run['run_id']])
                encoded = canonical(run)
                self.db.execute('INSERT INTO trace_runs VALUES(?,?,?,?)', (key, source_id, encoded, digest(encoded.encode())))
        return {'status': 'imported', 'source_id': source_id, 'runs': len(document['runs'])}

    def replay(self, source_id, document):
        source = self.db.execute('SELECT sha,policy FROM trace_sources WHERE id=?', (source_id,)).fetchone()
        policy = canonical({key: document[key] for key in ('normalizer_version', 'source_format', 'evidence_kind')})
        if source != (document['source_sha256'], policy):
            raise ValueError('replay source/provenance mismatch')
        expected = []
        for run in document['runs']:
            encoded = canonical(run)
            expected.append((canonical([run['harness'], run['route'], run['run_id']]), encoded, digest(encoded.encode())))
        actual = self.db.execute('SELECT id,normalized,sha FROM trace_runs WHERE source=? ORDER BY id', (source_id,)).fetchall()
        if actual != sorted(expected):
            raise ValueError('replay normalized corpus differs or is corrupt')
        return {'passed': True, 'runs': len(expected), 'normalizer_version': VERSION,
                'scope': 'deterministic normalization only; no provider/controller actions replayed'}

    def report(self):
        groups = {}
        for encoded, sha in self.db.execute('SELECT normalized,sha FROM trace_runs ORDER BY id'):
            if digest(encoded.encode()) != sha:
                raise ValueError('normalized observation digest mismatch')
            run = json.loads(encoded)
            key = (run['harness'], run['route'], run['evidence_kind'])
            group = groups.setdefault(key, {'harness': key[0], 'route': key[1], 'evidence_kind': key[2],
                'runs': 0, 'assessments': {}, 'checks_measured_runs': 0, 'executed': 0,
                'failed': 0, 'skipped': 0, 'cost_known_runs': 0, 'reported_cost_sum': 0})
            group['runs'] += 1
            grade = run['assessment']
            group['assessments'][grade] = group['assessments'].get(grade, 0) + 1
            if run['checks'] is not None:
                group['checks_measured_runs'] += 1
                for field in ('executed', 'failed', 'skipped'):
                    group[field] += run['checks'][field]
            if run['reported_cost'] is not None:
                group['cost_known_runs'] += 1
                group['reported_cost_sum'] += run['reported_cost']
        for group in groups.values():
            if group['checks_measured_runs'] == 0:
                for field in ('executed', 'failed', 'skipped'):
                    group[field] = None
            if group['cost_known_runs'] == 0:
                group['reported_cost_sum'] = None
        return {'normalizer_version': VERSION, 'authority': 'observation_only',
                'groups': [groups[key] for key in sorted(groups)],
                'readiness': 'not_assessed', 'live_model_qualification': False,
                'cost_scope': 'reported model API cost only; hardware/energy unmeasured'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['import', 'replay', 'report'])
    parser.add_argument('--db', required=True)
    parser.add_argument('--input')
    parser.add_argument('--source-id')
    parser.add_argument('--format', choices=['published-deliveries', 'sanitized-v1'])
    parser.add_argument('--evidence-kind', choices=sorted(KINDS))
    parser.add_argument('--reviewed-sanitized', action='store_true')
    args = parser.parse_args()
    ledger = None
    try:
        document = None
        if args.operation != 'report':
            if not all((args.input, args.source_id, args.format, args.evidence_kind, args.reviewed_sanitized)):
                raise ValueError('explicit input, source identity, format, kind and sanitization review acknowledgment required')
            document = normalize(read_reviewed(args.input), args.format, args.evidence_kind)
        ledger = Ledger(args.db, read_only=args.operation != 'import')
        result = (ledger.report() if args.operation == 'report' else
                  ledger.replay(args.source_id, document) if args.operation == 'replay' else
                  ledger.import_summary(args.source_id, document))
        print(canonical(result))
        return 0
    except (ValueError, TypeError, KeyError, RecursionError, OSError, sqlite3.Error):
        # Never echo rejected content, SQL rows, paths or provider diagnostics.
        print(canonical({'passed': False, 'error': 'trace operation rejected; check schema, bounds, provenance and local file access'}))
        return 1
    finally:
        if ledger is not None:
            ledger.close()


if __name__ == '__main__':
    raise SystemExit(main())
