"""Local fault injection with real native handoff CLI; only owned disposable children.
No model, actual productive worker, deployment, remote or shared-runtime operation.
"""
import pathlib,subprocess,json,hashlib,time,tempfile,os,sys,signal
B=pathlib.Path(sys.argv[1]).resolve();OUT=pathlib.Path(sys.argv[2]).resolve();OUT.mkdir(mode=0o700,exist_ok=True)
def put(p,v):p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(v))
def read(p):return json.loads(p.read_text())
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def identity(pid):
 p=pathlib.Path('/proc')/str(pid);f=(p/'stat').read_text().rsplit(')',1)[1].split();return {'pid':pid,'start_ticks':int(f[19]),'boot_id':pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'uid':p.stat().st_uid}
def wait_for(pred,timeout=12):
 until=time.monotonic()+timeout
 while time.monotonic()<until:
  if pred():return True
  time.sleep(.02)
 return False
records=[]
with tempfile.TemporaryDirectory(prefix='cf-native-handoff-') as td:
 for kind in ([sys.argv[3]] if len(sys.argv)>3 else ['crash-restart','separate-state-directory']):
  root=pathlib.Path(td)/kind;md=root/'manifest-dir';md.mkdir(parents=True);queue=root/'q';queue.mkdir(mode=0o700);goalroot=root/'goal';manual=root/'manual'
  for r in [goalroot,manual]:(r/'repository').mkdir(parents=True);put(r/'status.json',{'sequence':0,'current':None,'status':'paused'})
  (manual/'PAUSE').write_text('Explicit unrelated user pause')
  goals=[{'id':name,'root':str(r),'repository':str(r/'repository'),'cycle_command':['/bin/false'],'acceptance':{}} for name,r in [('goal',goalroot),('manual',manual)]]
  state=md/'state.json' if kind=='crash-restart' else root/'separate-state/state.json';mp=md/'manifest.json'
  manifest={'goals':goals,'state':str(state),'pool_lock':str(root/'pool.lock'),'worker_slots':1,'controller':str(B),'trusted_files':{str(B):sha(B)}};put(mp,manifest)
  active={'version':1,'cursor':0,'status':'running','current':{'goal':'goal','phase':'running'},'goals':{'goal':{'state':'ready','milestones':[]},'manual':{'state':'paused','milestones':[]}}};put(state,active)
  target=root/'replacement';marker=root/'replacement-started';target.write_text('#!/bin/sh\nprintf replacement > '+str(marker)+'\n');target.chmod(0o700);pins=root/'pins.json';put(pins,{'dispatcher':str(target),'files':{str(target):sha(target)}})
  owner=subprocess.Popen(['/bin/sleep','90']);helpers=[];files=[];record={'kind':kind,'scope':'Owned dummy coordinator only; no productive worker interrupted','source_binary_sha256':sha(B)}
  def start(n):
   log=(OUT/(kind+'-'+str(n)+'.log')).open('wb');files.append(log);p=subprocess.Popen([str(B),'queue-handoff','--manifest',str(mp),'--queue',str(queue),'--runtime-pins',str(pins)],stdout=log,stderr=log);helpers.append(p);return p
  try:
   put(queue/'owner.json',{'state':'active','process':identity(owner.pid)});first=start(1);hp=queue/'handoff.json'
   assert wait_for(lambda:hp.exists() and 'goal' in read(hp).get('owned_pauses',{})), 'first helper did not retain pause ownership'
   original=read(hp);pause=(goalroot/'PAUSE').read_text();record['first_record']=original;record['pause_created_by_actual_helper']=pause;record['productive_owner_preserved_while_active']=owner.poll() is None
   helper=first
   if kind=='crash-restart':
    first.kill();first.wait(timeout=5);record['injected_helper_exit']=first.returncode;assert owner.poll() is None;helper=start(2)
    assert wait_for(lambda:hp.exists() and read(hp).get('process',{}).get('pid')==helper.pid), 'replacement helper did not record its identity'
    record['restart_owned_pause_retained']='goal' in read(hp).get('owned_pauses',{});record['restart_record']=read(hp)
   paused={**active,'status':'paused','current':None,'goals':{'goal':{'state':'paused','milestones':[]},'manual':{'state':'paused','milestones':[]}}};put(state,paused)
   if kind=='separate-state-directory':
    woke=wait_for(lambda:owner.poll() is not None or helper.poll() is not None,4)
    record['actual_state_change_woke_handoff']=woke
    if not woke:
     record['owner_still_alive_before_unrelated_manifest_event']=owner.poll() is None
     (md/'unrelated-wakeup-control').write_text('Controlled unrelated directory event')
   assert wait_for(lambda:marker.exists() or helper.poll() is not None,15),'handoff did not finish even after controlled wake'
   helper.wait(timeout=5);owner.wait(timeout=5);record.update(helper_exit=helper.returncode,owner_exit=owner.returncode,replacement_started=marker.exists(),own_pause_released=not (goalroot/'PAUSE').exists(),unrelated_pause_unchanged=(manual/'PAUSE').read_text()=='Explicit unrelated user pause',final_record=read(hp))
  finally:
   for p in helpers+[owner]:
    if p.poll() is None:p.terminate()
   for p in helpers+[owner]:
    try:p.wait(timeout=5)
    except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
   for f in files:f.close()
  records.append(record)
(OUT/'observations.json').write_text(json.dumps({'scope':'Local real-native process fault fixtures only','records':records},indent=2)+'\n');print(json.dumps({'records':records}))
