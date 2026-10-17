"""Differential Python/Rust acceptance and real supervisor compatibility.

Python is the test oracle, never a dependency of the Rust execution path.
All repositories/processes are disposable; no provider, host runtime or network.
"""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER', ROOT / 'target/debug/code-factory-controller')).resolve()
spec = importlib.util.spec_from_file_location('legacy_acceptance', ROOT / 'scripts/goal-acceptance.py')
legacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(legacy)


def rust(operation, **kwargs):
    proc = subprocess.run([str(BINARY), 'goal-acceptance'], input=json.dumps(dict(operation=operation, **kwargs)), text=True, capture_output=True, timeout=20)
    if proc.returncode:
        raise ValueError(proc.stderr)
    return json.loads(proc.stdout)


def succeeds(function, *args, **kwargs):
    try:
        function(*args, **kwargs)
        return True
    except (ValueError, KeyError, TypeError):
        return False


class Policies(unittest.TestCase):
    def test_evidence_matrix_matches_python(self):
        for kind in ['cpu_tests', 'runtime_tests', 'coding_goal', 'vulkan_execution']:
            valid = {'passed': True, 'skipped': False, 'executed': 1, 'failed': 0, 'commit': 'a'*40, 'evidence_type': kind}
            if kind == 'vulkan_execution':
                valid.update(gpu_dispatches=1, validation_passed=True, device={'name': 'fixture', 'vendor_id': 1, 'device_id': 2})
            variants = [valid, None, [], True, {**valid, 'skipped_count': 0}]
            for key in valid:
                variants.append({k: v for k, v in valid.items() if k != key})
                for value in [None, False, True, 0, 0.0, 1, 1.0, -1, '', [], {}]:
                    variants.append({**valid, key: value})
            if kind == 'vulkan_execution':
                for name in [None, False, True, 0, 1, '', [], ['fixture'], {}, {'fixture': True}]:
                    variants.append({**valid,'device':{**valid['device'],'name':name}})
            for e in variants:
                with self.subTest(kind=kind, evidence=e):
                    check = {'evidence_type': kind}
                    self.assertEqual(succeeds(legacy.validate_evidence, e, check, 'a'*40), succeeds(rust, 'validate_evidence', evidence=e, check=check, commit='a'*40))

    def test_missing_and_malformed_suite_statuses_match_python(self):
        for key in ['completion_checks', 'required_evidence']:
            for value in [None, False, True, 0, 1, '', 'bad', [], {}, ['bad'], {'bad': True}]:
                manifest={'completion_checks':[{'id':'fixture','evidence_type':'fixture','argv':['/bin/true']}], 'required_evidence':['fixture'], key:value}
                with self.subTest(key=key,value=value):
                    old=legacy.run_acceptance(manifest,'unused-fixture-state','a'*40)
                    new=rust('run',manifest=manifest,state='unused-fixture-state',commit='a'*40)
                    self.assertEqual((new['passed'],new['status']),(old['passed'],old['status']))

    def test_policy_matrix_matches_python(self):
        valid = {'repository': '/tmp/fixture', 'required_evidence': ['cpu_tests'], 'completion_checks': [{'id': 'cpu', 'evidence_type': 'cpu_tests', 'argv': ['/usr/bin/true']}]}
        variants = [valid]
        for key in valid:
            variants.append({k: v for k, v in valid.items() if k != key})
            for value in [None, False, 1, '', [], {}]:
                variants.append({**valid, key: value})
        for key in ['argv', 'id', 'evidence_type', 'timeout_seconds']:
            for value in [None, False, True, 0, 0.1, 1, 1.1, 3600, 3601, '', [''], ['a\x00b'], ['/bin/true']]:
                changed = copy.deepcopy(valid)
                changed['completion_checks'][0][key] = value
                variants.append(changed)
        for env in [{'PATH':'/usr/bin'}, {'SECRET':'no'}, {'PATH':None}, [], {'CARGO_HOME':'a\0b'}]:
            variants.append({**valid, 'verification_environment':env})
        for manifest in variants:
            with self.subTest(manifest=manifest):
                self.assertEqual(succeeds(legacy.validate_policy, manifest), succeeds(rust, 'validate_policy', manifest=manifest))


class Execution(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root/'repo'
        self.repo.mkdir()
        subprocess.run(['git','init','-q',str(self.repo)], check=True)
        (self.repo/'tracked').write_text('approved\n')
        subprocess.run(['git','-C',str(self.repo),'add','.'], check=True)
        subprocess.run(['git','-C',str(self.repo),'-c','user.name=Fixture','-c','user.email=fixture@invalid','commit','-qm','baseline'],check=True)
        self.commit = subprocess.check_output(['git','-C',str(self.repo),'rev-parse','HEAD'],text=True).strip()
        # Deliberately uses only shell, git and the Rust binary at runtime.
        script = 'test "$(cat tracked)" = approved && test ! -e untracked && test -z "$FACTORY_OPERATOR_TOKEN" && test "$CI" = true && printf \'{"passed":true,"skipped":false,"executed":3,"failed":0,"commit":"%s","evidence_type":"goal_tests"}\\n\' "$FACTORY_VERIFICATION_COMMIT"'
        self.manifest = {'repository':str(self.repo), 'required_evidence':['goal_tests'], 'completion_checks':[{'id':'goal', 'evidence_type':'goal_tests', 'argv':['/bin/sh','-c',script]}], 'verification_environment':{'PATH':'/usr/bin:/bin'}}
        (self.repo/'tracked').write_text('dirty\n')
        (self.repo/'untracked').write_text('private')
        self.state = str(self.root/'state')

    def run_rust(self, manifest=None):
        return rust('run', manifest=manifest or self.manifest, state=self.state, commit=self.commit)

    def test_real_success_auditable_by_python_without_python_in_runtime(self):
        tools = self.root/'no-python-path'
        tools.mkdir()
        for name in ['git', 'cat']:
            (tools/name).symlink_to('/usr/bin/'+name)
        self.manifest['verification_environment']['PATH'] = str(tools)
        self.assertFalse((tools/'python3').exists())
        result = self.run_rust()
        self.assertTrue(result['passed'],result)
        self.assertEqual(result['receipts'][0]['executor']['quiescent'],True)
        self.assertEqual(legacy.audit_result(self.manifest,result,self.commit),result)
        self.assertEqual(rust('audit_result',manifest=self.manifest,result=result,commit=self.commit),result)
        self.assertEqual((self.repo/'tracked').read_text(),'dirty\n')

    def test_python_receipts_auditable_by_rust_including_legacy(self):
        for supervised in [False, True]:
            manifest = copy.deepcopy(self.manifest)
            if supervised:
                manifest['verifier_executable'] = str(BINARY)
            result = legacy.run_acceptance(manifest,self.root/str(supervised),self.commit)
            self.assertTrue(result['passed'],result)
            self.assertEqual(rust('audit_result',manifest=manifest,result=result,commit=self.commit),result)
            if not supervised:
                del result['receipts'][0]['stderr_sha256']
                self.assertEqual(rust('audit_result',manifest=manifest,result=result,commit=self.commit),result)
                with self.assertRaises(ValueError):
                    rust('audit_result',manifest={**manifest,'verifier_executable':str(BINARY)},result=result,commit=self.commit)

    def test_tampering_never_replays(self):
        result = self.run_rust()
        receipt = result['receipts'][0]
        stdout = Path(receipt['stdout'])
        paths = [stdout, Path(receipt['stderr']), stdout.with_suffix('.plan.json'), stdout.with_suffix('.process.json')]
        for path in paths:
            original = path.read_bytes()
            path.write_text('{}')
            with self.subTest(path=path), self.assertRaises(ValueError):
                rust('audit_result',manifest=self.manifest,result=result,commit=self.commit)
            path.write_bytes(original)
        self.assertEqual(rust('audit_result',manifest=self.manifest,result=result,commit=self.commit),result)

    def test_failure_timeout_and_oversize(self):
        for command,timeout in [('exit 7',5), ('sleep 30',1), ('head -c 1100000 /dev/zero',5), ('printf \'{"passed":true}\'',5)]:
            manifest = copy.deepcopy(self.manifest)
            manifest['completion_checks'][0].update(argv=['/bin/sh','-c',command],timeout_seconds=timeout)
            result = self.run_rust(manifest)
            self.assertFalse(result['passed'],result)
            self.assertEqual(result['phase'],'check_evidence')
            self.assertTrue(result['receipts'][0]['executor']['quiescent'])

    def test_unresolved_prior_plan_fences_new_execution(self):
        directory = Path(self.state+'.acceptance')/'prior'
        directory.mkdir(parents=True)
        (directory/'0.plan.json').write_text('{"execution_id":"prior"}')
        result = self.run_rust()
        self.assertTrue(result['execution_unresolved'],result)
        self.assertEqual(result['receipts'],[])
        self.assertEqual(len(list(directory.parent.iterdir())),1)

    def test_owner_loss_reaps_owned_descendants(self):
        marker = self.root/'child.pid'
        manifest = copy.deepcopy(self.manifest)
        manifest['completion_checks'][0].update(argv=['/bin/sh','-c',f'sleep 30 & echo $! > {marker}; wait'],timeout_seconds=30)
        p = subprocess.Popen([str(BINARY),'goal-acceptance'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            p.stdin.write(json.dumps(dict(operation='run',manifest=manifest,state=self.state,commit=self.commit)))
            p.stdin.close(); p.stdin=None
            until=time.monotonic()+5
            while not marker.exists() and time.monotonic()<until:time.sleep(.01)
            self.assertTrue(marker.exists())
            pid=marker.read_text().strip()
            p.terminate();p.communicate(timeout=5)
            until=time.monotonic()+5
            while Path('/proc',pid).exists() and time.monotonic()<until:time.sleep(.01)
            self.assertFalse(Path('/proc',pid).exists())
        finally:
            if p.poll() is None:p.kill();p.communicate(timeout=5)

    def test_missing_helper_receipt_never_falls_back(self):
        result=self.run_rust({**self.manifest,'verifier_executable':'/usr/bin/true'})
        self.assertFalse(result['passed']);self.assertTrue(result['execution_unresolved'])

if __name__ == '__main__': unittest.main(verbosity=2)
