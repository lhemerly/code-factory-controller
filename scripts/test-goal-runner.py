import importlib.util,json,os,pathlib,subprocess,sys,tempfile,unittest
from unittest import mock
s=importlib.util.spec_from_file_location('runner',pathlib.Path(__file__).with_name('goal-runner.py'))
m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Tests(unittest.TestCase):
 def test_resume_and_chained_base(self):
  jobs={};calls=[]
  def api(op,body):
   calls.append(op)
   if op=='list':return list(jobs.values())
   if op=='admit':jobs[body['job_id']]={'input':body,'state':'queued'};return {}
   j=jobs[body['job_id']]
   if op=='run':j['state']='done'
   if j['state']=='done':return {'state':'done','integrated_commit':'b'*40,'active_verification_receipt':{'outcome':'passed','candidate_commit':'b'*40},'delivery':{'commit':'b'*40}}
   return {'state':j['state'],'active_verification_receipt':None}
  with tempfile.TemporaryDirectory() as d:
   path=pathlib.Path(d)/'state.json';manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'},{'job_id':'two'}],'remaining_work':['broader runtime']}
   r=m.Runner(manifest,path,api);self.assertTrue(r.step())
   r=m.Runner(manifest,path,api);self.assertTrue(r.step());self.assertTrue(r.step())
   self.assertEqual(jobs['two']['input']['base_commit'],'b'*40)
   self.assertTrue(r.step());self.assertFalse(r.step());self.assertEqual(r.state['status'],'awaiting_plan')
   self.assertEqual(calls.count('admit'),2)
   manifest['tasks'][0]['job_id']='altered'
   with self.assertRaises(ValueError):m.Runner(manifest,path,api)
 def test_false_done_rejected(self):
  def api(op,b):
   if op=='list':return [{'input':{'job_id':'one','base_commit':'a'*40}}]
   return {'state':'done','integrated_commit':'b'*40,'active_verification_receipt':{'outcome':'passed','candidate_commit':'c'*40},'delivery':{'commit':'b'*40}}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]},pathlib.Path(d)/'state.json',api)
   with self.assertRaises(ValueError):r.step()
 def test_blocked_fenced(self):
  def api(op,b):
   if op=='list':return [{'input':{'job_id':'one','base_commit':'a'*40}}]
   return {'state':'blocked','blockers':['outcome_unknown']}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]},pathlib.Path(d)/'state.json',api)
   self.assertFalse(r.step());self.assertEqual(r.state['status'],'needs_attention')
 def test_failed_verification_resolves_before_requeue(self):
  calls=[]
  task={'job_id':'one','base_commit':'a'*40}
  def api(op,b):
   calls.append(op)
   if op=='list':return [{'input':task}]
   if op=='status':return {'state':'running','active_verification_receipt':{'outcome':'failed'}}
   return {}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}],'max_reworks':1},pathlib.Path(d)/'state.json',api)
   self.assertTrue(r.step());self.assertLess(calls.index('abort'),calls.index('requeue'))
   self.assertFalse(r.step());self.assertEqual(calls.count('abort'),1)
 def test_resume_after_abort_before_requeue(self):
  task={'job_id':'one','base_commit':'a'*40};calls=[]
  def api(op,b):
   calls.append(op)
   if op=='list':return [{'input':task,'attempt':{'dispatch_state':'resolved'}}]
   if op=='status':return {'state':'blocked'}
   return {}
  with tempfile.TemporaryDirectory() as d:
   path=pathlib.Path(d)/'state.json';manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]}
   r=m.Runner(manifest,path,api);r.state['current']={'input':task,'reworks':0,'repair_pending':True};r.persist()
   r=m.Runner(manifest,path,api);self.assertTrue(r.step());self.assertIn('requeue',calls);self.assertNotIn('abort',calls)
 def test_completed_delivery_reaudited_on_restart(self):
  def api(op,b):return {'state':'blocked','integrated_commit':'b'*40,'delivery':{'commit':'b'*40}}
  with tempfile.TemporaryDirectory() as d:
   path=pathlib.Path(d)/'state.json';manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]}
   r=m.Runner(manifest,path,api);r.state['completed']=[{'job_id':'one','integrated_commit':'b'*40,'delivery':{'commit':'b'*40}}];r.persist()
   r=m.Runner(manifest,path,api);self.assertFalse(r.step());self.assertEqual(r.state['audit_failure'],'one')
 def test_backlog_exhaustion_without_acceptance_never_completes(self):
  done={'state':'done','integrated_commit':'b'*40,'delivery':{'commit':'b'*40}}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}],'remaining_work':[]},pathlib.Path(d)/'state.json',lambda op,body:done)
   r.state['completed']=[{'job_id':'one','integrated_commit':'b'*40,'delivery':{'commit':'b'*40}}]
   self.assertFalse(r.step());self.assertEqual(r.state['status'],'awaiting_acceptance')
 def test_required_evidence_cannot_be_removed(self):
  with tempfile.TemporaryDirectory() as d:
   manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}],'required_evidence':['vulkan_execution']}
   path=pathlib.Path(d)/'state.json';m.Runner(manifest,path,lambda op,b:None)
   manifest['required_evidence']=[]
   with self.assertRaises(ValueError):m.Runner(manifest,path,lambda op,b:None)
class AcceptancePolicyTests(unittest.TestCase):
    def manifest(self, root):
        return {'goal_id': 'goal', 'base_commit': 'a' * 40,
                'repository': str(root / 'repository'),
                'tasks': [{'job_id': 'one'}],
                'verification_environment': {'PATH': '/usr/bin:/bin', 'RUSTUP_TOOLCHAIN': 'stable'},
                'required_evidence': ['cpu_tests'],
                'completion_checks': [{'id': 'cpu', 'evidence_type': 'cpu_tests', 'argv': ['/usr/bin/true']}]}

    def done(self, runner):
        runner.state['completed'] = [{'job_id': 'one', 'integrated_commit': 'b' * 40,
                                      'delivery': {'commit': 'b' * 40}}]
        runner.persist()

    def api(self, op, body):
        return {'state': 'done', 'integrated_commit': 'b' * 40, 'delivery': {'commit': 'b' * 40}}

    def test_repository_and_environment_cannot_change_on_resume(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            path = root / 'state.json'
            manifest = self.manifest(root)
            original = m.Runner(manifest, path, self.api)
            snapshot = path.read_text()
            for field, value in [('repository', str(root / 'other')),
                                 ('verification_environment', {'PATH': '/different', 'RUSTUP_TOOLCHAIN': 'stable'}),
                                 ('verification_environment', {'PATH': '/usr/bin:/bin', 'RUSTUP_TOOLCHAIN': 'nightly'}),
                                 ('verification_environment', {'PATH': '/usr/bin:/bin', 'VK_DRIVER_FILES': '/different/icd.json'})]:
                with self.subTest(field=field, value=value):
                    changed = {**manifest, field: value}
                    with self.assertRaisesRegex(ValueError, 'repository/environment cannot change'):
                        m.Runner(changed, path, self.api)
                    self.assertEqual(path.read_text(), snapshot)
            resumed = m.Runner(manifest, path, self.api)
            self.assertEqual(resumed.state['acceptance_policy_digest'], original.state['acceptance_policy_digest'])

    def test_default_path_is_frozen_and_appended_checks_update_policy_digest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            manifest = self.manifest(root)
            manifest.pop('verification_environment')
            path = root / 'state.json'
            with mock.patch.dict(os.environ, {'PATH': '/approved/bin'}):
                original = m.Runner(manifest, path, self.api)
                original.state['status'] = 'completed'
                original.persist()
                manifest['completion_checks'].append({'id': 'extra', 'evidence_type': 'cpu_tests', 'argv': ['/usr/bin/true']})
                extended = m.Runner(manifest, path, self.api)
                self.assertNotEqual(extended.state['acceptance_policy_digest'], original.state['acceptance_policy_digest'])
                self.assertEqual(extended.state['status'], 'awaiting_acceptance')
                self.assertEqual(extended.state['unmet_evidence'], ['cpu_tests'])
            with mock.patch.dict(os.environ, {'PATH': '/different/bin'}):
                with self.assertRaisesRegex(ValueError, 'repository/environment cannot change'):
                    m.Runner(manifest, path, self.api)

    def test_legacy_state_is_snapshotted_and_old_acceptance_is_historical(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            path = root / 'state.json'
            manifest = self.manifest(root)
            original = m.Runner(manifest, path, self.api)
            self.done(original)
            legacy = json.loads(path.read_text())
            legacy.pop('acceptance_context')
            legacy.pop('acceptance_policy_digest')
            legacy['status'] = 'completed'
            legacy['acceptance'] = {'passed': True, 'status': 'passed', 'commit': 'b' * 40}
            m.save(path, legacy)
            resumed = m.Runner(manifest, path, self.api)
            self.assertEqual(resumed.state['status'], 'awaiting_acceptance')
            self.assertNotIn('acceptance', resumed.state)
            self.assertTrue(resumed.state['legacy_acceptance']['passed'])
            self.assertEqual(resumed.state['acceptance_policy_migration']['kind'], 'legacy_inputs_first_snapshot')
            self.assertEqual(resumed.state['completed'], legacy['completed'])
            self.assertEqual(json.loads(path.read_text())['status'], 'awaiting_acceptance')
            self.assertFalse(resumed.step())
            self.assertEqual(resumed.state['status'], 'acceptance_failed')

    def test_setup_failure_is_durable_and_retryable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            path = root / 'state.json'
            manifest = self.manifest(root)
            runner = m.Runner(manifest, path, self.api)
            self.done(runner)
            self.assertFalse(runner.step())
            saved = json.loads(path.read_text())
            self.assertEqual(saved['status'], 'acceptance_failed')
            self.assertEqual(saved['acceptance']['phase'], 'setup')
            self.assertEqual(saved['acceptance']['commit'], 'b' * 40)
            self.assertEqual(saved['acceptance']['policy_digest'], saved['acceptance_policy_digest'])
            self.assertIn('retry_guidance', saved['acceptance'])
            resumed = m.Runner(manifest, path, self.api)
            self.assertFalse(resumed.step())
            self.assertEqual(json.loads(path.read_text())['status'], 'acceptance_failed')

    def test_final_acceptance_uses_snapshotted_path_after_host_environment_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            repository = root / 'repository'
            repository.mkdir()
            subprocess.run(['git', 'init', '-q', str(repository)], check=True)
            subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                            'commit', '--allow-empty', '-qm', 'baseline'], cwd=repository, check=True)
            commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repository, text=True).strip()
            manifest = self.manifest(root)
            manifest.pop('verification_environment')
            manifest['base_commit'] = commit
            manifest['completion_checks'][0]['argv'] = [sys.executable, '-c',
                "import os,json; assert os.environ['PATH']=='/usr/bin:/bin'; print(json.dumps({'passed':True,'skipped':False,'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'cpu_tests'}))"]
            done = {'state': 'done', 'integrated_commit': commit, 'delivery': {'commit': commit}}
            path = root / 'state.json'
            with mock.patch.dict(os.environ, {'PATH': '/usr/bin:/bin'}):
                runner = m.Runner(manifest, path, lambda op, body: done)
                runner.state['completed'] = [{'job_id': 'one', 'integrated_commit': commit, 'delivery': done['delivery']}]
                with mock.patch.dict(os.environ, {'PATH': '/changed-after-start'}):
                    self.assertFalse(runner.step())
            saved = json.loads(path.read_text())
            self.assertEqual(saved['status'], 'completed')
            self.assertTrue(saved['acceptance']['passed'])
            self.assertEqual(saved['acceptance']['commit'], commit)

    def test_unexpected_acceptance_loader_error_cannot_leave_running_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            path = root / 'state.json'
            runner = m.Runner(self.manifest(root), path, self.api)
            self.done(runner)
            with mock.patch.object(m.importlib.util, 'spec_from_file_location', side_effect=RuntimeError('bad\x1b' * 1000)):
                self.assertFalse(runner.step())
            saved = json.loads(path.read_text())
            self.assertEqual(saved['status'], 'acceptance_failed')
            self.assertEqual(saved['acceptance']['phase'], 'acceptance_runner')
            self.assertLessEqual(len(saved['acceptance']['reason']), 1000)
            self.assertNotIn('\x1b', saved['acceptance']['reason'])

if __name__=='__main__':unittest.main()
