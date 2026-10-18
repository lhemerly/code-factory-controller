"""Real handoff persistence/user-pause/owner-death faults, disposable children only."""
import pathlib,subprocess,json,hashlib,time,tempfile,sys,fcntl
binary=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2]);out.mkdir();kind=sys.argv[3]
def put(p,v):p.write_text(json.dumps(v))
def read(p):return json.loads(p.read_text())
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def identity(pid):
 p=pathlib.Path('/proc')/str(pid);s=(p/'stat').read_text().rsplit(')',1)[1].split();return {'pid':pid,'start_ticks':int(s[19]),'boot_id':pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'uid':p.stat().st_uid}
def wait(pred,seconds=8):
 until=time.monotonic()+seconds
 while time.monotonic()<until:
  if pred():return True
  time.sleep(.01)
 return False
records=[]
for variant in (['replace','inplace'] if kind=='user-pause' else [kind]):
 with tempfile.TemporaryDirectory(prefix='native-handoff-journal-') as d:
  r=pathlib.Path(d);q=r/'q';q.mkdir(mode=0o700);goal=r/'goal';manual=r/'manual'
  for g in [goal,manual]:(g/'repository').mkdir(parents=True);put(g/'status.json',{'sequence':0,'current':None,'status':'paused'})
  (manual/'PAUSE').write_text('Existing user pause');state=r/'state.json';mp=r/'manifest.json';marker=r/'replacement-ran';replacement=r/'replacement';replacement.write_text('#!/bin/sh\nprintf ok > '+str(marker)+'\n');replacement.chmod(0o700)
  goals=[{'id':n,'root':str(g),'repository':str(g/'repository'),'cycle_command':['/bin/false'],'acceptance':{}} for n,g in [('goal',goal),('manual',manual)]]
  if kind=='bounded':goals[0]['admission_mode']='one_cycle'
  m={'goals':goals,'state':str(state),'pool_lock':str(r/'pool.lock'),'worker_slots':1,'controller':str(binary),'trusted_files':{str(binary):sha(binary)}};put(mp,m);pins=r/'pins.json';put(pins,{'dispatcher':str(replacement),'files':{str(replacement):sha(replacement)}})
  active={'version':1,'cursor':0,'status':'running','current':{'goal':'goal','phase':'running'},'goals':{'goal':{'state':'ready','milestones':[]},'manual':{'state':'paused','milestones':[]}}};put(state,active)
  # A real pinned native owner publishes its own canonical launch binding.
  # Hold this disposable pool to prevent admission while the fixture controls
  # the existing state-boundary and crash cases below.
  pool=(r/'pool.lock').open('a');fcntl.flock(pool,fcntl.LOCK_EX)
  owner_log=(out/(variant+'-owner.log')).open('wb')
  owner=subprocess.Popen([str(binary),'research-dispatch','run','--manifest',str(mp),'--queue',str(q),'--wait-for-owner'],stdout=owner_log,stderr=owner_log)
  assert wait(lambda:(q/'owner.json').exists() and read(q/'owner.json').get('state')=='waiting_for_boundary'),'native owner did not publish its bound identity'
  helper=None;log=(out/(variant+'.log')).open('wb');record={'variant':variant}
  try:
   if kind=='journal-failure':(q/'handoff.json').mkdir()
   helper=subprocess.Popen([str(binary),'queue-handoff','--manifest',str(mp),'--queue',str(q),'--runtime-pins',str(pins)],stdout=log,stderr=log)
   if kind=='journal-failure':
    helper.wait(timeout=8);record.update(failed_before_pause=helper.returncode!=0 and not (goal/'PAUSE').exists(),owner_preserved=owner.poll() is None,replacement_started=marker.exists())
   else:
    assert wait(lambda:(goal/'PAUSE').exists() and (q/'handoff.json').exists()),'helper did not publish its pause'
    if kind=='user-pause':
     text='User override while maintenance waits'
     if variant=='replace':p=goal/'user-pause';p.write_text(text);p.replace(goal/'PAUSE')
     else:(goal/'PAUSE').write_text(text)
     paused={**active,'status':'paused','current':None,'goals':{'goal':{'state':'paused','milestones':[]},'manual':{'state':'paused','milestones':[]}}};put(state,paused);owner.wait(timeout=8);pool.close();helper.wait(timeout=12)
     record.update(helper_exit=helper.returncode,user_pause_preserved=(goal/'PAUSE').read_text()==text,goal_stays_paused=read(state)['goals']['goal']['state']=='paused',no_goal_resume_event=not (goal/'events.ndjson').exists(),replacement_started=marker.exists())
    elif kind=='bounded':
     # The bounded cycle completed while its maintenance request was pending.
     put(goal/'status.json',{'sequence':1,'current':None,'status':'waiting'})
     paused={**active,'status':'paused','current':None,'goals':{'goal':{'state':'paused','milestones':[],'native_status':'accepted','reason':'Bounded implementation cycle ended; independent review or explicit repair admission required'},'manual':{'state':'paused','milestones':[]}}};put(state,paused)
     owner.wait(timeout=8);pool.close();helper.wait(timeout=12)
     record.update(helper_exit=helper.returncode,bounded_scope_preserved=read(state)['goals']['goal']['state']=='paused',owned_pause_detached=not (goal/'PAUSE').exists(),replacement_started=marker.exists())
    else:
     # Crash only the directly owned dummy coordinator; no state file changes.
     owner.kill();owner.wait(timeout=5);pool.close();woke=wait(lambda:helper.poll() is not None,4)
     record.update(owner_death_woke_handoff=woke,replacement_started=marker.exists(),uncertain_admission_fenced=woke and helper.returncode!=0)
   record['unrelated_pause_preserved']=(manual/'PAUSE').read_text()=='Existing user pause'
  finally:
   for p in [helper,owner]:
    if p is not None and p.poll() is None:p.terminate()
   for p in [helper,owner]:
    if p is not None:
     try:p.wait(timeout=5)
     except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
   log.close();owner_log.close();pool.close()
  records.append(record)
(out/'observations.json').write_text(json.dumps({'scope':'Local actual-native fault fixtures only; no provider or shared owner','records':records},indent=2)+'\n');print(json.dumps(records))
