"""Actual dispatcher CLI, supervised baseline checks and deterministic coding; no models."""
import contextlib,hashlib,json,os,pathlib,subprocess,sys,tempfile,time
ROOT=pathlib.Path(__file__).resolve().parent
binary=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',ROOT.parent/'target/debug/code-factory-controller')).resolve()
evidence=os.environ.get('FACTORY_BASELINE_EVIDENCE_DIR');started=time.monotonic()
if evidence:pathlib.Path(evidence).mkdir(parents=True,exist_ok=False)
with contextlib.nullcontext(evidence) if evidence else tempfile.TemporaryDirectory(prefix='goal-baseline-') as temp:
 directory=pathlib.Path(temp);goals=[]
 checker=directory/'trusted-goal.py'
 checker.write_text("import json,os,pathlib\np=pathlib.Path('result.txt');assert p.exists() and p.read_text()=='verified result\\n', 'Unicode normalization still missing'\nprint(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'commit':os.environ['FACTORY_VERIFICATION_COMMIT'],'evidence_type':'coding_goal'}))\n")
 for name in ['already-satisfied','needs-work']:
  root=directory/name;repo=root/'repository';repo.mkdir(parents=True)
  (repo/'README.md').write_text('Frozen fixture baseline\n');(root/'task.txt').write_text('Implement required behavior while preserving frozen tests.')
  if name=='already-satisfied':(repo/'result.txt').write_text('verified result\n')
  subprocess.run(['git','init','-q',str(repo)],check=True);subprocess.run(['git','-C',str(repo),'add','.'],check=True)
  subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','baseline'],check=True)
  base=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip()
  (root/'goal.json').write_text(json.dumps({'id':name,'objective':'Implement the approved result behavior','completion_criteria':['Exact result independently checked'],'pause_conditions':['Ownership or permission uncertainty']}))
  command=['/usr/bin/env','FACTORY_FIXTURE_CONTROLLER='+str(binary),'FACTORY_FIXTURE_ADAPTIVE=1',sys.executable,str(ROOT/'fake-research-cycle.py'),str(root),'--one-cycle']
  goals.append({'id':name,'root':str(root),'repository':str(repo),'initial_base':base,'cycle_command':command,'between_cycles_seconds':0,'acceptance':{'required_evidence':['coding_goal'],'completion_checks':[{'id':'required-behavior','argv':[sys.executable,str(checker)],'evidence_type':'coding_goal'}]}})
 manifest={'goals':goals,'controller':str(binary),'worker_slots':1,'pool_lock':str(directory/'pool.lock'),'state':str(directory/'dispatch.json'),'trusted_files':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'research-dispatch.py',ROOT/'fake-research-cycle.py',ROOT/'goal-acceptance.py',ROOT/'research-context.mjs',checker,binary]}}
 path=directory/'launch.json';path.write_text(json.dumps(manifest));argv=[sys.executable,str(ROOT/'research-dispatch.py'),'run','--manifest',str(path)]
 run=subprocess.run(argv,capture_output=True,text=True,timeout=45);assert run.returncode==0,(run.stdout,run.stderr)
 state=json.loads((directory/'dispatch.json').read_text());assert state['status']=='complete',state
 done=state['goals']['already-satisfied'];work=state['goals']['needs-work']
 assert done['state']=='complete' and not done['milestones'] and done['completion_base']==goals[0]['initial_base']
 assert not (directory/'already-satisfied/status.json').exists()
 assert work['state']=='complete' and len(work['milestones'])==1
 context=json.loads((directory/'needs-work/cycles/000001/admission-context.json').read_text())
 assert 'Unicode normalization still missing' in context['text']
 assert context['provenance']['goal_evaluation_sha256']
 repeat=subprocess.run(argv,capture_output=True,text=True,timeout=30);assert repeat.returncode==0,repeat.stderr
 assert json.loads((directory/'needs-work/status.json').read_text())['sequence']==1
 # Completion from the existing baseline is backed by real supervised execution,
 # not a cached text claim: restart rejects modified independent evidence.
 receipt=done['goal_acceptance']['receipts'][0];assert receipt['executor']['quiescent']
 pathlib.Path(receipt['stdout']).write_text('forged')
 tampered=subprocess.run(argv,capture_output=True,text=True,timeout=30);assert tampered.returncode!=0
 summary={'passed':True,'evidence':'actual local dispatcher CLI; deterministic coding worker; no inference','already_satisfied_goal_worker_dispatches':0,'unmet_goal_worker_dispatches':1,'initial_failure_guided_first_worker':True,'independent_candidate_and_integration_receipts':2,'baseline_completion_reaudited':True,'tampered_baseline_evidence_rejected':True,'restart_without_duplicate_work':True,'elapsed_seconds':round(time.monotonic()-started,3),'model_tokens':None,'model_cache_tokens':None}
 if evidence:(directory/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
 print(json.dumps(summary,indent=2))
