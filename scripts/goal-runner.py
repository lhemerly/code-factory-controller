#!/usr/bin/env python3
"""Durable sequential operator for an approved implementation backlog."""
import argparse
import importlib.util
import datetime
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import tempfile
import time
import urllib.request
from urllib.parse import urlsplit


def save(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(dir=path.parent, prefix='.goal-')
    try:
        with os.fdopen(fd, 'w') as stream:
            json.dump(value, stream, indent=2)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


class Runner:
    def __init__(self, manifest, state_path, api):
        self.manifest, self.path, self.api = manifest, Path(state_path), api
        assert manifest['goal_id'] and manifest['tasks']
        ids = [t['job_id'] for t in manifest['tasks']]
        if len(set(ids)) != len(ids):
            raise ValueError('duplicate job IDs')
        self.state = json.loads(self.path.read_text()) if self.path.exists() else {
            'goal_id': manifest['goal_id'], 'completed': [], 'current': None,
            'status': 'running', 'task_hashes': [], 'initial_base': manifest['base_commit']}
        hashes = [digest(t) for t in manifest['tasks']]
        old = self.state['task_hashes']
        if (self.state['goal_id'] != manifest['goal_id'] or hashes[:len(old)] != old
                or self.state['initial_base'] != manifest['base_commit']):
            raise ValueError('existing approved tasks/base/goal cannot change; append tasks instead')
        required = set(manifest.get('required_evidence', []))
        if not set(self.state.get('required_evidence', [])).issubset(required):
            raise ValueError('required completion evidence cannot be removed')
        completion_hashes = [digest(c) for c in manifest.get('completion_checks', [])]
        old_checks = self.state.get('completion_check_hashes', [])
        if completion_hashes[:len(old_checks)] != old_checks:
            raise ValueError('approved completion checks cannot be removed or changed')
        self.state['required_evidence'] = sorted(required)
        self.state['completion_check_hashes'] = completion_hashes
        self.state['task_hashes'] = hashes
        self.audit_required = True
        self.persist()

    def persist(self):
        self.state['updated_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        save(self.path, self.state)

    def step(self):
        index = len(self.state['completed'])
        if self.audit_required or index == len(self.manifest['tasks']):
            for completed in self.state['completed']:
                audited = self.api('status', {'job_id': completed['job_id']})
                if (audited['state'] != 'done'
                        or audited.get('integrated_commit') != completed['integrated_commit']
                        or audited.get('delivery') != completed['delivery']):
                    self.state['status'] = 'needs_attention'
                    self.state['audit_failure'] = completed['job_id']
                    self.persist()
                    return False
            self.audit_required = False
        if index == len(self.manifest['tasks']):
            self.state['remaining_work'] = self.manifest.get('remaining_work', [])
            self.state['unmet_evidence'] = self.state['required_evidence']
            if self.state['remaining_work']:
                self.state['status'] = 'awaiting_plan'
            else:
                # Backlog exhaustion is never sufficient for goal acceptance.
                spec = importlib.util.spec_from_file_location('goal_acceptance', Path(__file__).with_name('goal-acceptance.py'))
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                commit = self.state['completed'][-1]['integrated_commit']
                self.state['status'] = 'acceptance_running'
                self.persist()
                acceptance = module.run_acceptance(self.manifest, self.path, commit)
                self.state['acceptance'] = acceptance
                self.state['status'] = 'completed' if acceptance['passed'] else acceptance['status']
                if acceptance['passed']:
                    self.state['unmet_evidence'] = []
            self.persist()
            return False
        if not self.state['current']:
            base = (self.state['completed'][-1]['integrated_commit']
                    if index else self.manifest['base_commit'])
            task = dict(self.manifest['tasks'][index], base_commit=base)
            self.state['current'] = {'input': task, 'reworks': 0}
            # Persist the exact admission intent before the HTTP effect.
            self.persist()
        current = self.state['current']
        task = current['input']
        known = next((j for j in self.api('list', {}) if j['input']['job_id'] == task['job_id']), None)
        if known is None:
            self.api('admit', task)
        elif known['input'] != task:
            raise ValueError('controller job identity has a different approved input')
        status = self.api('status', {'job_id': task['job_id']})
        current['last_status'] = status
        if current.get('repair_pending'):
            if status['state'] == 'queued':
                current['reworks'] += 1
                current.pop('repair_pending', None)
            elif status['state'] == 'blocked' and known and (known.get('attempt') or {}).get('dispatch_state') == 'resolved':
                self.api('requeue', {'job_id': task['job_id']})
                current['reworks'] += 1
                current.pop('repair_pending', None)
            else:
                self.state['status'] = 'needs_attention'
                self.persist()
                return False
        elif status['state'] == 'done':
            receipt, delivery = status.get('active_verification_receipt'), status.get('delivery')
            if (not receipt or receipt['outcome'] != 'passed' or not delivery
                    or receipt['candidate_commit'] != status['integrated_commit']
                    or delivery['commit'] != status['integrated_commit']):
                raise ValueError('done result lacks exact integrated verification/delivery')
            self.state['completed'].append({'job_id': task['job_id'],
                'integrated_commit': status['integrated_commit'], 'delivery': delivery, 'receipt': receipt})
            self.state['current'] = None
        elif status['state'] == 'queued':
            self.api('run', {'job_id': task['job_id'], 'mode': 'adapter'})
        elif status['state'] == 'retry_wait':
            # Controller owns provider cooldown, fencing, and attempt limits.
            self.api('run', {'job_id': task['job_id'], 'mode': 'adapter'})
        elif status['state'] in ('blocked', 'paused'):
            self.state['status'] = 'needs_attention'
            self.persist()
            return False
        elif (status['state'] == 'running' and (status.get('active_verification_receipt') or {}).get('outcome') == 'failed'):
            # Explicitly resolve the worker before retrying. Never retry an unknown effect.
            if current['reworks'] >= self.manifest.get('max_reworks', 1):
                self.state['status'] = 'needs_attention'
                self.persist()
                return False
            current['repair_pending'] = True
            self.persist()
            self.api('abort', {'job_id': task['job_id']})
            self.api('requeue', {'job_id': task['job_id']})
            current['reworks'] += 1
            current.pop('repair_pending', None)
        self.state['status'] = 'running'
        self.persist()
        return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', required=True)
    parser.add_argument('--state', required=True)
    parser.add_argument('--url', default='http://127.0.0.1:4317')
    parser.add_argument('--max-seconds', type=int, default=86400)
    parser.add_argument('--poll-seconds', type=float, default=4)
    args = parser.parse_args()
    url = urlsplit(args.url)
    import ipaddress
    try:
        loopback = ipaddress.ip_address(url.hostname).is_loopback
    except ValueError:
        loopback = False
    if (url.scheme != 'http' or not loopback
            or url.path or url.query or url.fragment or url.username or url.password):
        parser.error('controller URL must be a numeric loopback HTTP origin')
    if args.max_seconds <= 0 or not math.isfinite(args.poll_seconds) or args.poll_seconds <= 0:
        parser.error('time limits must be positive')
    token = os.environ.get('FACTORY_OPERATOR_TOKEN', '')
    if len(token) < 32:
        parser.error('FACTORY_OPERATOR_TOKEN must contain at least 32 characters')
    def api(op, body):
        request = urllib.request.Request(args.url + '/operator/' + op,
            data=json.dumps(body).encode(), headers={
                'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)
    state_path = Path(args.state)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    with open(str(state_path) + '.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        runner = Runner(json.loads(Path(args.manifest).read_text()), state_path, api)
        deadline = time.monotonic() + args.max_seconds
        previous = None
        while time.monotonic() < deadline:
            continuing = runner.step()
            progress = (runner.state['status'], len(runner.state['completed']),
                runner.state['current']['input']['job_id'] if runner.state['current'] else None)
            if progress != previous:
                print(json.dumps({'status':progress[0], 'completed':progress[1], 'current':progress[2]}), flush=True)
                previous = progress
            if not continuing:
                return 0 if progress[0] == 'completed' else 2
            time.sleep(args.poll_seconds)
        runner.state['status'] = 'budget_exhausted'
        runner.persist()
        print('Run budget exhausted; resume with the same manifest/state. Active job is retained.', flush=True)
        return 75

if __name__ == '__main__':
    raise SystemExit(main())
