"""Local deterministic worker, real controller and whole-goal evaluation, no inference."""
import contextlib, hashlib, importlib.util, json, os, pathlib, subprocess, tempfile, time
ROOT = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('dispatcher', ROOT/'research-dispatch.py')
D = importlib.util.module_from_spec(spec)
spec.loader.exec_module(D)
binary = pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER', ROOT.parent/'target/debug/code-factory-controller')).resolve()
started = time.monotonic()
evidence = os.environ.get('FACTORY_FEEDBACK_EVIDENCE_DIR')
if evidence:
    pathlib.Path(evidence).mkdir(parents=True, exist_ok=False)
with contextlib.nullcontext(evidence) if evidence else tempfile.TemporaryDirectory(prefix='factory-goal-feedback-') as temp:
    root = pathlib.Path(temp)
    repo = root/'repository'
    repo.mkdir()
    (repo/'README.md').write_text('Frozen fixture baseline\n')
    subprocess.run(['git','init','-q',str(repo)],check=True)
    subprocess.run(['git','-C',str(repo),'add','.'],check=True)
    subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','baseline'],check=True)
    goal = {'id':'coding-feedback','objective':'Implement the approved result behavior','completion_criteria':['Exact result independently checked'],'pause_conditions':['Ownership or policy uncertainty']}
    (root/'goal.json').write_text(json.dumps(goal))
    (root/'task.txt').write_text('Implement the required result, preserving frozen tests.')
    checker = root/'trusted-goal.py'
    checker.write_text("import os,json,pathlib\nassert pathlib.Path('result.txt').read_text()=='verified result\\n', 'Unicode normalization still missing'\nprint(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'evidence_type':'coding_goal','commit':os.environ['FACTORY_VERIFICATION_COMMIT']}))\n")
    entry = {'id':goal['id'],'root':str(root),'repository':str(repo),'cycle_command':['/usr/bin/python3',str(ROOT/'fake-research-cycle.py'),str(root),'--one-cycle'],'between_cycles_seconds':0,'acceptance':{'required_evidence':['coding_goal'],'completion_checks':[{'id':'required-result','argv':['/usr/bin/python3',str(checker)],'evidence_type':'coding_goal'}]}}
    manifest = {'goals':[entry],'controller':str(binary),'worker_slots':1,'pool_lock':str(root/'pool.lock'),'state':str(root/'dispatch.json')}
    env = {**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary),'FACTORY_FIXTURE_ADAPTIVE':'1'}
    crash_test = os.environ.get('FACTORY_FIXTURE_TEST_HOST_EXIT')=='1'
    if crash_test:
        entry['cycle_command']=['/usr/bin/env','FACTORY_FIXTURE_CONTROLLER='+str(binary),'FACTORY_FIXTURE_ADAPTIVE=1','FACTORY_FIXTURE_EXIT_AFTER_SAVE=7',*entry['cycle_command']]
    def worker(g,record):
        subprocess.run(g['cycle_command'],cwd=repo,env=env,check=True,capture_output=True,text=True,timeout=45)
    d = D.Dispatcher(manifest,root/'dispatch.json',runner=None if crash_test else worker)
    d.pool_fds=()
    d.step()
    first = d.s['goals'][goal['id']]
    assert first['state']=='ready' and len(first['milestones'])==1
    assert first['goal_acceptance']['status']=='acceptance_failed'
    failed = json.loads((root/'goal-evaluation.json').read_text())
    receipt = failed['result']['receipts'][0]
    assert 'Unicode normalization still missing' in pathlib.Path(receipt['stderr']).read_text()
    assert hashlib.sha256(pathlib.Path(receipt['stderr']).read_bytes()).hexdigest()==receipt['stderr_sha256']
    # Restart before repair, re-audit first accepted milestone and consume its
    # independent failure through the same context builder used by pstack.
    d = D.Dispatcher(manifest,root/'dispatch.json',runner=None if crash_test else worker)
    d.pool_fds=()
    d.recover_and_audit()
    d.step()
    final = d.s['goals'][goal['id']]
    assert final['state']=='complete' and len(final['milestones'])==2
    if crash_test:assert final['last_host_exit']['exit_status']==7
    context = json.loads((root/'cycles/000002/admission-context.json').read_text())
    assert 'Unicode normalization still missing' in context['text']
    assert context['provenance']['goal_evaluation_sha256']==hashlib.sha256(json.dumps(failed,indent=2).encode()+b'\n').hexdigest()
    d = D.Dispatcher(manifest,root/'dispatch.json',runner=lambda *_: (_ for _ in ()).throw(AssertionError('duplicate dispatch')))
    d.recover_and_audit()
    assert not d.step()
    summary = {'passed':True,'evidence':'local deterministic worker; no model/provider calls','accepted_controller_deliveries':2,'independent_candidate_and_integration_receipts':4,'whole_goal_evaluations':2,'goal_failure_consumed_after_restart':True,'completed_without_operator_repair':True,'restart_without_duplicate_dispatch':True,'nonzero_host_exit_recovered':crash_test,'tokens':None,'cache_tokens':None,'elapsed_seconds':round(time.monotonic()-started,3)}
    if evidence:
        (root/'evidence-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print(json.dumps(summary,indent=2))
