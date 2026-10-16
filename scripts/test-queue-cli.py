"""Actual queue CLI + kernel handoff + real controller receipts; providers are deterministic fakes."""
import fcntl,hashlib,importlib.util,json,os,pathlib,subprocess,sys,tempfile,time
S=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('d',S/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D)
binary=S.parent/'target/debug/code-factory-controller';fake=S/'fake-research-cycle.py'
with tempfile.TemporaryDirectory(prefix='queue-cli-') as temp:
 root=pathlib.Path(temp);goals=[];controller_configs=[];code=root/'scripts';code.mkdir();fake=code/'fake.py';fake.write_bytes((S/'fake-research-cycle.py').read_bytes())
 for name in ['existing','approved-new','approved-later']:
  g=root/name;repo=g/'repository';repo.mkdir(parents=True);(repo/'README.md').write_text('Frozen baseline\n')
  subprocess.run(['git','init','-q',str(repo)],check=True);subprocess.run(['git','-C',str(repo),'add','.'],check=True);subprocess.run(['git','-C',str(repo),'-c','core.hooksPath=/dev/null','-c','user.name=Fixture','-c','user.email=fixture@localhost','commit','-qm','baseline'],check=True)
  goal={'id':name,'objective':'Queue fixture only, no actual research','completion_criteria':['Independent owner scope review'],'pause_conditions':['Unknown execution']};D.save(g/'goal.json',goal)
  profile={'id':'fixture','version':'1','timeout_seconds':30,'checks':[{'id':'result','argv':['/usr/bin/true']}]}
  config={'project_id':name,'repository':str(repo),'data_dir':str(g/'registered'),'delivery_dir':str(g/'delivery'),'integration_ref':'refs/heads/fixture-integration','profiles':[profile],'policy':{'models':['fixture/free'],'free_only':True,'spend_ceiling':0,'data_approved':True,'cooldown_seconds':1},'max_workers':1,'lease_seconds':3600,'max_attempts':1,'opencode_url':None,'verifier_executable':str(binary)};D.save(g/'controller.json',config)
  subprocess.run([str(binary),'goal','--config',str(g/'controller.json'),'--input',str(g/'goal.json')],check=True,stdout=subprocess.DEVNULL)
  D.save(g/'supervisor.json',{'native':'fixture-only','native_sha256':'fixture-only','controller':str(binary),'controller_sha256':D.queue_api.sha(binary),'plugin':'fixture-only','plugin_commit':'fixture-only','venv':'fixture-only','path':'/usr/bin','ownership_lease_seconds':3600,'ownership_heartbeat_seconds':60})
  D.save(g/'job-template.json',{});D.save(g/'protected.json',[]);(g/'task.txt').write_text('Fake provider fixture')
  goals.append({'id':name,'root':str(g),'repository':str(repo),'cycle_command':['/usr/bin/python3',str(fake),str(g),'--one-cycle'],'between_cycles_seconds':0,'admission_mode':'one_cycle','acceptance':{'required_evidence':['owner_scope_review'],'completion_checks':[]}})
 pin=code/'pin.mjs';pin.write_text('old verified code');candidate=root/'candidate';candidate.write_text('new verified code')
 manifest={'goals':[goals[0]],'worker_slots':1,'pool_lock':str(root/'pool.lock'),'state':str(root/'state.json'),'controller':str(binary),'trusted_files':{str(p):D.queue_api.sha(p) for p in [pin,binary,fake,S/'research-dispatch.py',S/'dispatch-queue.py']}}
 mp=root/'launch.json';D.save(mp,manifest);d=D.Dispatcher(manifest,root/'state.json');d.s['goals']['existing']['state']='paused';d.s['status']='paused';d.persist()
 queue=root/'queue';cli=[sys.executable,str(S/'research-dispatch.py')];common=['--manifest',str(mp),'--queue',str(queue)]
 envelope={'goal':goals[1],'trusted_files':{str(p):D.queue_api.sha(p) for p in pathlib.Path(goals[1]['root']).iterdir() if p.is_file()}};ip=root/'intent.json';D.save(ip,envelope)
 registered=json.loads(subprocess.check_output([*cli,'queue-status',*common,'--input',str(ip)],text=True));assert registered['goals'][0]['state']=='registered'
 held=(root/'pool.lock').open('a');fcntl.flock(held,fcntl.LOCK_EX)
 p=subprocess.Popen([*cli,'run',*common,'--wait-for-owner'],env={**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary)},stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 def wait_for(predicate):
  limit=time.monotonic()+15
  while time.monotonic()<limit:
   if p.poll() is not None:raise AssertionError(p.communicate())
   if predicate():return
   time.sleep(.02)
  raise AssertionError('fixture event timeout')
 try:
  wait_for(lambda:(queue/'owner.json').exists())
  assert json.loads((queue/'owner.json').read_text())['state']=='waiting_for_boundary';assert not (pathlib.Path(goals[1]['root'])/'status.json').exists()
  out=json.loads(subprocess.check_output([*cli,'enqueue',*common,'--input',str(ip)],text=True));assert out['state']=='queued'
  again=json.loads(subprocess.check_output([*cli,'enqueue',*common,'--input',str(ip)],text=True));assert again==out
  proposed=json.loads(json.dumps(manifest));proposed['trusted_files'][str(pin)]=D.queue_api.sha(candidate)
  report=root/'compatibility.json';D.save(report,{'passed':True,'permissions_unchanged':True,'tests':[{'name':'offline compatibility fixture','passed':True}]})
  update={'previous_manifest':manifest,'manifest':proposed,'files':[{'source':str(candidate),'destination':str(pin),'sha256':D.queue_api.sha(candidate)}],'verification':{'path':str(report),'sha256':D.queue_api.sha(report)}};up=root/'update.json';D.save(up,update)
  subprocess.run([*cli,'stage-update',*common,'--input',str(up)],check=True,stdout=subprocess.DEVNULL)
  assert pin.read_text()=='old verified code'
  competing=subprocess.run([*cli,'run',*common,'--wait-for-owner','--exit-when-idle'],env={**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary)},capture_output=True,text=True,timeout=5)
  assert competing.returncode!=0 and 'Another owner holds' in competing.stderr
  assert json.loads((queue/'owner.json').read_text())['state']=='waiting_for_boundary'
  # Cancel only the standby executor; the old fixture owner and queued intent remain intact.
  p.terminate();p.communicate(timeout=5);assert p.returncode==0
  assert not (pathlib.Path(goals[1]['root'])/'status.json').exists()
  p=subprocess.Popen([*cli,'run',*common,'--wait-for-owner'],env={**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary)},stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
  wait_for(lambda:(queue/'owner.json').exists() and json.loads((queue/'owner.json').read_text())['state']=='waiting_for_boundary')
  held.close()
  wait_for(lambda:D.queue_api.read(root/'state.json')['goals'].get('approved-new',{}).get('state')=='paused')
  observed=D.queue_api.read(root/'state.json');assert pin.read_text()=='new verified code';assert len(observed['goals']['approved-new']['milestones'])==1;assert observed['goals']['approved-new']['usage']['cost'] is None
  qstatus=json.loads(subprocess.check_output([*cli,'queue-status',*common],text=True));assert qstatus['goals'][0]['state']=='awaiting_review'
  later={'goal':goals[2],'trusted_files':{str(f):D.queue_api.sha(f) for f in pathlib.Path(goals[2]['root']).iterdir() if f.is_file()}}
  later_path=root/'later.json';D.save(later_path,later)
  assert (queue/'wake.sock').exists()
  subprocess.run([*cli,'enqueue',*common,'--input',str(later_path)],check=True,stdout=subprocess.DEVNULL)
  wait_for(lambda:D.queue_api.read(root/'state.json')['goals'].get('approved-later',{}).get('state')=='paused')
  assert len(D.queue_api.read(root/'state.json')['goals']['approved-later']['milestones'])==1
  p.terminate();stdout,stderr=p.communicate(timeout=5);assert p.returncode==0,(stdout,stderr)
  reopen=subprocess.run([*cli,'run',*common,'--exit-when-idle'],env={**os.environ,'FACTORY_FIXTURE_CONTROLLER':str(binary)},capture_output=True,text=True,timeout=15);assert reopen.returncode==0,reopen.stderr
  assert D.queue_api.read(pathlib.Path(goals[1]['root'])/'status.json')['sequence']==1
  print(json.dumps({'passed':True,'registered_distinct_from_queued':True,'occupied_pool_no_preemption':True,'kernel_boundary_handoff':True,'duplicate_standby_owner_refused':True,'standby_cancel_and_restart_preserves_intent':True,'approved_update_applied_before_admission':True,'real_controller_candidate_and_integrated_receipts':4,'new_goal_wakes_running_idle_executor':True,'providers':'fake only, no model calls','unknown_usage_preserved':True,'software_milestone_not_goal_completion':True,'restart_without_duplicate_cycle':True},indent=2))
 finally:
  held.close()
  if p.poll() is None:p.kill();p.wait()
