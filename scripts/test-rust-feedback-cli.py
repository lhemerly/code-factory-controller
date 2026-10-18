"""Native continuous repair loop, real controller + whole-goal checks, fake coding only."""
import importlib.util,os,pathlib
S=pathlib.Path(__file__).resolve().parent
sp=importlib.util.spec_from_file_location('native_fixture',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(sp);sp.loader.exec_module(helper);fixture_temp,binary=helper.native_binary(S.parent);os.environ['FACTORY_FIXTURE_CONTROLLER']=str(binary)
source=(S/'test-goal-feedback-cli.py').read_text();source=source[:source.index('    def worker(g,record):')]
source+='''    manifest['trusted_files']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [binary,ROOT/'fake-research-cycle.py',ROOT/'research-context.mjs',checker]}
    if not crash_test:entry['cycle_command']=['/usr/bin/env','FACTORY_FIXTURE_CONTROLLER='+str(binary),'FACTORY_FIXTURE_ADAPTIVE=1',*entry['cycle_command']]
    mp=root/'manifest.json';mp.write_text(json.dumps(manifest));argv=[str(binary),'research-dispatch','run','--manifest',str(mp)]
    run=subprocess.run(argv,capture_output=True,text=True,env=env,timeout=60);assert run.returncode==0,(run.stdout,run.stderr)
    state=json.loads((root/'dispatch.json').read_text());final=state['goals'][goal['id']]
    assert state['status']=='complete' and final['state']=='complete',state
    assert len(final['milestones'])==2 and final['goal_acceptance']['passed'] is True
    context=json.loads((root/'cycles/000002/admission-context.json').read_text())
    assert 'Unicode normalization still missing' in context['text']
    assert context['provenance']['goal_evaluation_sha256']
    if crash_test:assert final['last_host_exit']['exit_status']==7
    repeat=subprocess.run(argv,capture_output=True,text=True,env=env,timeout=30);assert repeat.returncode==0,repeat.stderr
    assert json.loads((root/'status.json').read_text())['sequence']==2
    print(json.dumps({'passed':True,'evidence':'actual native CLI continuous repair; deterministic coding; no model inference','accepted_deliveries':2,'whole_goal_evaluations':2,'independent_failure_drives_repair':True,'restart_without_duplicate_work':True,'nonzero_host_exit_recovered':crash_test,'tokens':None,'cache_tokens':None,'elapsed_seconds':round(time.monotonic()-started,3)},indent=2))
'''
exec(compile(source,str(S/'test-goal-feedback-cli.py'),'exec'),{'__file__':str(S/'test-goal-feedback-cli.py'),'__name__':'__main__'})
