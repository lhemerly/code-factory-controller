"""Actual owner launcher CLI, fake providers, real controller/independent receipts."""
import hashlib,json,os,pathlib,subprocess,tempfile,sys
root=pathlib.Path(__file__).resolve().parent;binary=root.parent/'target/debug/code-factory-controller';dispatcher=root/'research-dispatch.py';fake=root/'fake-research-cycle.py'
with tempfile.TemporaryDirectory(prefix='two-goal-cli-') as temp:
 directory=pathlib.Path(temp);goals=[];env={**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary)}
 checker=directory/'trusted-completion.py';checker.write_text("import pathlib,json,os\nassert pathlib.Path('result.txt').read_text()=='verified result\\n'\nprint(json.dumps({'passed':True,'skipped':False,'executed':1,'failed':0,'evidence_type':'fixture_science','commit':os.environ['FACTORY_VERIFICATION_COMMIT']}))\n")
 for name in ['scroll-fixture','ink-fixture']:
  goalroot=directory/name;repo=goalroot/'repository';repo.mkdir(parents=True);(repo/'README.md').write_text('Frozen fixture baseline\n')
  subprocess.run(['git','init','-q',str(repo)],check=True);subprocess.run(['git','-C',str(repo),'add','.'],check=True);subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','baseline'],check=True)
  (goalroot/'goal.json').write_text(json.dumps({'id':name,'objective':'Synthetic dispatcher fixture; not scroll or ink evidence','completion_criteria':['Exact fixture artifact independently checked'],'pause_conditions':['Fixture failure']}))
  goals.append({'id':name,'root':str(goalroot),'repository':str(repo),'cycle_command':['/usr/bin/python3',str(fake),str(goalroot),'--one-cycle'],'between_cycles_seconds':0,'acceptance':{'required_evidence':['fixture_science'],'completion_checks':[{'id':'fixture-goal','argv':['/usr/bin/python3',str(checker)],'evidence_type':'fixture_science'}]}})
 manifest={'goals':goals,'controller':str(binary),'worker_slots':1,'pool_lock':str(directory/'pool.lock'),'state':str(directory/'dispatch.json'),'trusted_files':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [dispatcher,fake,checker,binary]}}
 path=directory/'launch.json';path.write_text(json.dumps(manifest));argv=['/usr/bin/python3',str(dispatcher),'run','--manifest',str(path),'--resume']
 run=subprocess.run(argv,capture_output=True,text=True,env=env,timeout=60)
 assert run.returncode==0,(run.stdout,run.stderr,{g['id']:(pathlib.Path(g['root'])/'dispatcher-console.log').read_text() for g in goals if (pathlib.Path(g['root'])/'dispatcher-console.log').exists()})
 state=json.loads((directory/'dispatch.json').read_text());assert state['status']=='complete',state
 assert all(s['state']=='complete' and len(s['milestones'])==1 for s in state['goals'].values())
 # Reopening audits stored real controller receipts and does not resubmit either fake provider.
 repeat=subprocess.run(argv,capture_output=True,text=True,env=env,timeout=30);assert repeat.returncode==0,repeat.stderr
 assert all(json.loads((pathlib.Path(g['root'])/'status.json').read_text())['sequence']==1 for g in goals)
 # Corrupted delivery makes the real controller reject the stored success before any new work.
 milestone=state['goals']['scroll-fixture']['completion_milestone'];out=json.loads(pathlib.Path(milestone['result']).read_text());archive=pathlib.Path(out['acceptance']['delivery']['destination']);archive.write_bytes(archive.read_bytes()+b'tampered')
 corrupt=subprocess.run(argv,capture_output=True,text=True,env=env,timeout=30);assert corrupt.returncode!=0
 print(json.dumps({'passed':True,'provider':'fake only; no native model calls','goals':2,'real_controller_candidate_and_integrated_receipts':4,'independent_clean_checkout_goal_checks':2,'restart_without_duplicate_dispatch':True,'tampered_delivery_refused':True,'actual_cli_executed':argv[:3]+['--manifest','<temporary fixture manifest>','--resume']},indent=2))
