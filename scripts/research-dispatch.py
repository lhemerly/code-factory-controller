#!/usr/bin/env python3
"""Foreground, one-slot operator for existing Code Factory goal-cycle runners.
Native execution and receipts remain authoritative in the existing runtime/controller.
"""
import argparse, datetime, fcntl, hashlib, importlib.util, json, os, pathlib, signal, subprocess, sys, time
ROOT=pathlib.Path(__file__).resolve().parent

def module(name,file):
 spec=importlib.util.spec_from_file_location(name,ROOT/file);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
save=module('goal_storage','goal-runner.py').save
acceptance=module('final_goal_acceptance','goal-acceptance.py')
queue_api=module('dispatch_intents','dispatch-queue.py')

def utc():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def digest(v):return hashlib.sha256(json.dumps(v,sort_keys=True).encode()).hexdigest()
def identity(pid):
 try:
  p=pathlib.Path('/proc')/str(pid);s=(p/'stat').read_text();start=int(s.rsplit(')',1)[1].split()[19]);return {'pid':pid,'start_ticks':start,'boot_id':pathlib.Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'uid':p.stat().st_uid,'argv':(p/'cmdline').read_bytes().split(b'\0')[:-1]}
 except (FileNotFoundError,ProcessLookupError):return None

def same_process(record):
 current=identity(record.get('pid',-1));return current is not None and all(current[k]==record[k] for k in ['pid','start_ticks','boot_id','uid'])

def merge_circuits(*sources):
 result={}
 for source in sources:
  for model,circuit in source.items():
   if model not in result or circuit['until']>result[model]['until']:result[model]=circuit
 return result

def migrate_launch_policy(manifest,state_path,migration_path):
 """Explicit owner repair of pinned implementation; goal/permission contracts stay exact."""
 state_path=pathlib.Path(state_path);state=json.loads(state_path.read_text());migration=json.loads(pathlib.Path(migration_path).read_text());previous=migration['previous_manifest']
 if migration.get('kind')!='lifecycle-recovery-repair' or migration.get('from')!=state['manifest_digest'] or digest(previous)!=migration['from'] or digest(manifest)!=migration.get('to'):raise ValueError('Repair migration identity mismatch')
 if {k:v for k,v in previous.items() if k not in ['controller','trusted_files']}!={k:v for k,v in manifest.items() if k not in ['controller','trusted_files']}:raise ValueError('Repair cannot change goals, scope, commands, pricing or acceptance contracts')
 if state.get('current',{} ) and state['current'].get('process') and same_process(state['current']['process']):raise RuntimeError('Cannot migrate a live owner')
 for file,expected in manifest['trusted_files'].items():
  if hashlib.sha256(pathlib.Path(file).read_bytes()).hexdigest()!=expected:raise ValueError('Repair pin mismatch: '+file)
 locks=Locks()
 try:
  for goal in manifest['goals']:locks.acquire(pathlib.Path(goal['root'])/'supervisor.lock')
  state['manifest_digest']=digest(manifest);state.setdefault('implementation_migrations',[]).append({'kind':migration['kind'],'from':migration['from'],'to':migration['to'],'at':utc(),'reason':migration['reason']});save(state_path,state)
 finally:locks.close()

class Locks:
 def __init__(self):self.files=[]
 def acquire(self,path,wait=False):
  p=pathlib.Path(path);p.parent.mkdir(parents=True,exist_ok=True);f=p.open('a')
  try:fcntl.flock(f,fcntl.LOCK_EX|(0 if wait else fcntl.LOCK_NB))
  except BlockingIOError:f.close();raise RuntimeError('Another owner holds '+str(p))
  os.set_inheritable(f.fileno(),True);self.files.append(f);return f
 def close(self):
  for f in reversed(self.files):f.close()
  self.files=[]
 def fds(self):return tuple(f.fileno() for f in self.files)

class Dispatcher:
 def __init__(self,manifest,state_path,runner=None,audit=None,complete=None,clock=time.time):
  self.m=manifest;self.path=pathlib.Path(state_path);self.clock=clock;self.runner=runner or self.run_cycle;self.audit=audit or self.audit_cycle;self.complete=complete or self.complete_goal
  for file,expected in manifest.get('trusted_files',{}).items():
   if hashlib.sha256(pathlib.Path(file).read_bytes()).hexdigest()!=expected:raise ValueError('Trusted launcher/verifier bytes changed: '+file)
  goals=manifest['goals'];ids=[g['id'] for g in goals]
  if not ids or len(set(ids))!=len(ids):raise ValueError('Goal IDs must be unique')
  repos=[pathlib.Path(g['repository']).resolve() for g in goals];roots=[pathlib.Path(g['root']).resolve() for g in goals]
  if len(set(repos))!=len(repos) or len(set(roots))!=len(roots):raise ValueError('Goals must have distinct repositories and state roots')
  for a in [*repos,*roots]:
   for b in [*repos,*roots]:
    if any(a==root and b==repo or b==root and a==repo for root,repo in zip(roots,repos)):continue
    if a!=b and (a in b.parents or b in a.parents):raise ValueError('Goal repositories overlap')
  if manifest.get('worker_slots')!=1:raise ValueError('This dispatcher owns one shared worker slot')
  self.s=json.loads(self.path.read_text()) if self.path.exists() else {'version':1,'cursor':0,'goals':{g['id']:{'state':'ready','milestones':[]} for g in goals},'current':None,'status':'ready'}
  fingerprint=digest(manifest)
  if self.s.get('manifest_digest',fingerprint)!=fingerprint:raise ValueError('Approved dispatcher manifest changed; explicit migration required')
  self.s['manifest_digest']=fingerprint
  self.s['circuits']=merge_circuits(self.s.get('circuits',{}),*(self.read_status(g).get('circuits',{}) for g in goals))
  self.persist()
 def persist(self):self.s['updated_utc']=utc();save(self.path,self.s)
 def select(self):
  goals=self.m['goals'];start=self.s['cursor']%len(goals)
  for offset in range(len(goals)):
   index=(start+offset)%len(goals);g=goals[index];s=self.s['goals'][g['id']]
   if s['state'] not in ('paused','complete') and s.get('next_at',0)<=self.clock():return index,g
  return None
 def recover(self):
  record=self.s.get('current')
  if not record:return
  if record.get('process') and same_process(record['process']):raise RuntimeError('Existing cycle owner is alive; do not duplicate')
  goal=next(g for g in self.m['goals'] if g['id']==record['goal'])
  status=self.read_status(goal)
  if status.get('sequence')!=record['expected_sequence'] or status.get('current') or not status.get('last_cycle'):
   self.s['status']='needs_attention';self.persist();raise RuntimeError('Interrupted admission/cycle is uncertain; preserve evidence, no automatic dispatch')
  self.finish(goal,status);self.s['cursor']=(record['index']+1)%len(self.m['goals']);self.s['current']=None;self.persist()
 def read_status(self,goal):
  p=pathlib.Path(goal['root'])/'status.json';return json.loads(p.read_text()) if p.exists() else {'sequence':0,'current':None,'status':'starting'}
 def audit_cycle(self,goal,out):
  if out.get('goal_id')!=goal['id']:raise RuntimeError('Saved receipt belongs to another goal')
  if out.get('cleanup_confirmed') is not True:raise RuntimeError('Cycle lacks positive process cleanup')
  if out['status']!='accepted':
   if out['status']=='all_free_unavailable':return None
   recovery=out.get('recovery') or {}
   if recovery.get('execution_stop',{}).get('status')!='confirmed' or recovery.get('accounting',{}).get('status')=='violating':raise RuntimeError('Unconfirmed execution stop or known policy violation; no replacement')
   config=pathlib.Path(out['_result_path']).parent/'controller.json'
   observed=subprocess.run([self.m['controller'],'inspect','--config',str(config),'--job',out['job_id']],capture_output=True,text=True,check=True)
   current=json.loads(observed.stdout).get('execution_recovery') or {}
   if current.get('stop_id')!=recovery.get('stop_id') or current.get('execution_stop',{}).get('status')!='confirmed' or current.get('accounting',{}).get('status')=='violating':raise RuntimeError('Controller did not confirm exact failed-attempt stop; no replacement')
   return None
  config=pathlib.Path(out['_result_path']).parent/'controller.json'
  p=subprocess.run([self.m['controller'],'inspect','--config',str(config),'--job',out['job_id']],capture_output=True,text=True,check=True)
  authoritative=json.loads(p.stdout);receipt=authoritative.get('active_verification_receipt') or {}
  if authoritative.get('state')!='done' or authoritative.get('outcome')!='verified_delivery' or receipt.get('outcome')!='passed' or authoritative.get('integrated_commit')!=out['acceptance']['integrated_commit'] or receipt.get('verification_id')!=out['acceptance']['active_verification_receipt']['verification_id']:raise RuntimeError('Controller did not confirm exact passing integrated delivery')
  delivery=authoritative['delivery'];archive=pathlib.Path(delivery['destination'])
  if hashlib.sha256(archive.read_bytes()).hexdigest()!=delivery['sha256']:raise RuntimeError('Delivered artifact digest mismatch')
  return {'job':out['job_id'],'commit':authoritative['integrated_commit'],'receipt':receipt['verification_id'],'result':out['_result_path']}
 def complete_goal(self,goal,milestone):
  manifest={**goal['acceptance'],'repository':goal['repository']}
  result=acceptance.run_acceptance(manifest,self.path.parent/(goal['id']+'-goal'),milestone['commit'])
  # The existing clean-checkout independent acceptance implementation supplies receipts.
  return result
 def finish(self,goal,status):
  gs=self.s['goals'][goal['id']]
  if status.get('current'):gs['state']='paused';raise RuntimeError('Goal has unresolved active state; cannot reassign')
  last=status.get('last_cycle')
  if not last:
   gs['state']='paused';gs['reason']=status.get('pause_reason','No completed cycle');return
  p=pathlib.Path(last['result']);out=json.loads(p.read_text());out['_result_path']=str(p)
  if out.get('sequence')!=status['sequence'] or out.get('goal_id')!=goal['id']:raise RuntimeError('Cycle/goal identity mismatch')
  milestone=self.audit(goal,out)
  gs['last_result']=str(p);gs['usage']=out.get('usage');gs['model']=out.get('model');gs['native_status']=out.get('status');gs['harness']=goal['cycle_command']
  self.s['circuits']=merge_circuits(self.s.get('circuits',{}),status.get('circuits',{}))
  if self.m.get('circuits_file'):save(pathlib.Path(self.m['circuits_file']),self.s['circuits'])
  if milestone:
   if not any(m['job']==milestone['job'] for m in gs['milestones']):gs['milestones'].append(milestone)
   result=self.complete(goal,milestone);gs['goal_acceptance']=result
   if result.get('passed') is True and result.get('status')=='passed' and result.get('commit')==milestone['commit'] and result.get('receipts'):
    gs['state']='complete';gs['completion_milestone']=milestone;return
  if goal.get('admission_mode')=='one_cycle' and out['status']!='all_free_unavailable':
   gs['state']='paused';gs['reason']='Bounded implementation cycle ended; independent review or explicit repair admission required';return
  if status.get('status')=='paused':gs['state']='paused';gs['reason']=status.get('pause_reason');return
  gs['state']='ready';gs['next_at']=self.clock()+goal.get('between_cycles_seconds',1)
  if out['status']=='all_free_unavailable':
   gs['state']='waiting';gs['next_at']=max(self.clock()+1,datetime.datetime.fromisoformat(status['next_check_utc'].replace('Z','+00:00')).timestamp())
 def step(self):
  selected=self.select()
  if selected is None:
   states=[s['state'] for s in self.s['goals'].values()];self.s['status']='complete' if all(s=='complete' for s in states) else 'waiting' if any(s not in ('complete','paused') for s in states) else 'paused';self.persist();return False
  index,goal=selected;prior=self.read_status(goal)
  if self.m.get('circuits_file'):save(pathlib.Path(self.m['circuits_file']),self.s['circuits'])
  self.s['status']='running';self.s['current']={'goal':goal['id'],'index':index,'expected_sequence':prior['sequence']+1,'phase':'spawning'};self.persist()
  try:
   self.runner(goal,self.s['current']);status=self.read_status(goal)
   if status['sequence']!=self.s['current']['expected_sequence']:
    if status['sequence']==self.s['current']['expected_sequence']-1 and status.get('status')=='paused' and not status.get('current'):
     self.s['goals'][goal['id']].update(state='paused',reason=status.get('pause_reason','Paused before admission'))
    else:raise RuntimeError('Expected one new completed cycle')
   else:self.finish(goal,status)
  except Exception as error:
   self.s['status']='needs_attention';self.s['goals'][goal['id']]['state']='paused';self.s['goals'][goal['id']]['reason']=str(error);self.persist();raise
  self.s['cursor']=(index+1)%len(self.m['goals']);self.s['current']=None;self.persist();return True
 def run_cycle(self,goal,record):
  locks=Locks();locks.acquire(pathlib.Path(goal['root'])/'supervisor.lock')
  try:
   command=goal['cycle_command'];log=pathlib.Path(goal['root'])/'dispatcher-console.log'
   with log.open('ab') as stream:
    p=subprocess.Popen(command,cwd=goal['repository'],stdin=subprocess.DEVNULL,stdout=stream,stderr=stream,pass_fds=(*self.pool_fds,*locks.fds()),start_new_session=True)
    record['process']={k:v for k,v in identity(p.pid).items() if k!='argv'};record['phase']='running';self.persist()
    try:code=p.wait() # No productive-research elapsed cutoff.
    except BaseException:
     p.send_signal(signal.SIGTERM)
     try:p.wait(timeout=120) # Operator-stop cleanup only, never a research budget.
     except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait()
     raise
    if code:raise RuntimeError('Goal cycle exited '+str(code)+'; inspect '+str(log))
  finally:locks.close()
 def audit_completed(self):
  for g in self.m['goals']:
   gs=self.s['goals'][g['id']]
   for milestone in gs.get('milestones',[]):
    out=json.loads(pathlib.Path(milestone['result']).read_text());out['_result_path']=milestone['result']
    if self.audit(g,out)!=milestone:raise RuntimeError('Stored milestone identity changed')
   if gs['state']!='complete':continue
   result=gs['goal_acceptance']
   for receipt in result.get('receipts',[]):
    path=pathlib.Path(receipt['stdout'])
    if hashlib.sha256(path.read_bytes()).hexdigest()!=receipt['stdout_sha256']:raise RuntimeError('Goal acceptance evidence changed; completion unconfirmed')

 def recover_and_audit(self):
  try:self.recover();self.audit_completed()
  except Exception as error:
   self.s['status']='needs_attention';self.s['reason']=str(error);self.persist();raise

def owner_resume(manifest):
 # Called only by an explicit owner launcher --resume, never by a model/worker.
 old=manifest.get('legacy_idle_owner')
 if old and same_process(old):
  current=identity(old['pid']);g=manifest['goals'][0];s=json.loads((pathlib.Path(g['root'])/'status.json').read_text())
  expected=[b.encode() for b in old['required_argv']]
  if current['uid']!=os.getuid() or s.get('current') or s.get('status')!='paused' or any(a not in current['argv'] for a in expected):raise RuntimeError('Legacy owner is not the exact idle paused supervisor')
  os.kill(old['pid'],signal.SIGTERM)
  until=time.monotonic()+15
  while same_process(old) and time.monotonic()<until:time.sleep(.1)
  if same_process(old):raise RuntimeError('Legacy supervisor has not stopped')
 for goal in manifest['goals']:
  root=pathlib.Path(goal['root']);status=root/'status.json'
  if not status.exists():continue
  s=json.loads(status.read_text())
  if s.get('current'):raise RuntimeError('Cannot resume unresolved '+goal['id'])
  if s.get('last_cycle'):
   result=json.loads(pathlib.Path(s['last_cycle']['result']).read_text())
   if result.get('cleanup_confirmed') is not True:raise RuntimeError('Cannot resume without confirmed process cleanup')
   if result.get('status') not in ('accepted','all_free_unavailable'):
    recovery=result.get('recovery') or {}
    if result.get('fence_error') or recovery.get('execution_stop',{}).get('status')!='confirmed' or recovery.get('accounting',{}).get('status')=='violating':raise RuntimeError('Cannot resume unconfirmed stop or known policy violation')
  if s.get('status')=='paused' or (root/'PAUSE').exists():
   subprocess.run([sys.executable,str(ROOT/'spark-control.py'),'resume','--root',str(root),'--reason','Owner explicitly launched the shared two-goal foreground dispatcher; inspect prior owned-stop evidence and continue preserved checkpoints'],check=True)

def registered_goal(controller,config):
 return json.loads(subprocess.check_output([controller,'goal','--config',str(config)],text=True,timeout=30))

def inspect_job(controller,config,job):
 return json.loads(subprocess.check_output([controller,'inspect','--config',str(config),'--job',job],text=True,timeout=40))

def main():
 p=argparse.ArgumentParser();p.add_argument('action',choices=['run','status','enqueue','stage-update','queue-status','retry']);p.add_argument('--manifest',required=True,type=pathlib.Path);p.add_argument('--resume',action='store_true');p.add_argument('--policy-migration',type=pathlib.Path);p.add_argument('--queue',type=pathlib.Path);p.add_argument('--input',type=pathlib.Path);p.add_argument('--goal');p.add_argument('--wait-for-owner',action='store_true');p.add_argument('--exit-when-idle',action='store_true');a=p.parse_args();m=json.loads(a.manifest.read_text());state=pathlib.Path(m['state'])
 queue=queue_api.Queue(a.queue,save) if a.queue else None
 if a.action in ['enqueue','stage-update','queue-status','retry']:
  if not queue:p.error('--queue required')
  if a.action=='retry':
   if not a.goal:p.error('--goal required')
   item=queue_api.request_retry(queue,m,a.goal,inspect_job,Locks);print(json.dumps({'state':'retry_queued','request':item['id'],'goal':item['goal']}));return
  if a.action=='enqueue':
   if not a.input:p.error('--input required')
   item=queue.enqueue(json.loads(a.input.read_text()),m,registered_goal);print(json.dumps({'state':'queued','intent':item['id'],'goal':item['input']['goal']['id']}));return
  if a.action=='stage-update':
   if not a.input:p.error('--input required')
   item=queue.stage(json.loads(a.input.read_text()));print(json.dumps({'state':'staged','update':item['id']}));return
  observed=json.loads(state.read_text()) if state.exists() else {};items=queue.describe(observed)
  if a.input:
   envelope=json.loads(a.input.read_text());name=envelope['goal']['id']
   if not any(i['id']==name for i in items):
    queue_api.validate_goal(envelope,m,registered_goal);items.append({'id':name,'state':'registered','intent':None})
  print(json.dumps({'goals':items,'owner':queue_api.read(queue.root/'owner.json') if (queue.root/'owner.json').exists() else None},indent=2));return
 if a.action=='status':print(state.read_text() if state.exists() else json.dumps({'status':'not_started'}));return
 if not m.get('trusted_files'):raise ValueError('Owner launcher requires a pinned trusted_files manifest')
 if a.wait_for_owner and not queue:p.error('--wait-for-owner requires --queue')
 def stopping(signum,frame):raise KeyboardInterrupt('Owner requested stop')
 signal.signal(signal.SIGTERM,stopping)
 locks=Locks();queue_lock=Locks();receiver=None
 try:
  if queue:
   queue_lock.acquire(queue.root/'owner.lock')
   save(queue.root/'owner.json',{'state':'waiting_for_boundary','process':{k:v for k,v in identity(os.getpid()).items() if k!='argv'},'dispatcher_sha256':queue_api.sha(pathlib.Path(__file__))})
  locks.acquire(m['pool_lock'],wait=a.wait_for_owner) # Kernel wait, no polling and no second worker.
  m=json.loads(a.manifest.read_text());state=pathlib.Path(m['state'])
  if queue:
   journal=queue.root/'update-journal.json'
   if journal.exists():queue_api.restore(queue,queue_api.read(journal));m=queue_api.read(a.manifest)
   journal=queue.root/'enroll-journal.json'
   if journal.exists():
    saved=queue_api.read(journal);save(a.manifest,saved['manifest']);save(state,saved['state']);queue_api.unlink_durable(journal);m=saved['manifest']
  if a.policy_migration:migrate_launch_policy(m,state,a.policy_migration)
  dispatcher=Dispatcher(m,state);dispatcher.pool_fds=locks.fds()
  dispatcher.recover_and_audit()
  if queue:
   queue_api.boundary_updates(queue,a.manifest,state,Locks,audit=lambda proposed:Dispatcher(proposed,state).recover_and_audit())
   m=queue_api.read(a.manifest);dispatcher=Dispatcher(m,state);dispatcher.pool_fds=locks.fds();dispatcher.recover_and_audit();receiver=queue.receiver()
   save(queue.root/'owner.json',{'state':'active','process':{k:v for k,v in identity(os.getpid()).items() if k!='argv'},'dispatcher_sha256':queue_api.sha(pathlib.Path(__file__))})
  if a.resume:owner_resume(m)
  if a.resume:
   for goal in m['goals']:
    gs=dispatcher.s['goals'][goal['id']]
    if gs['state']=='paused' and dispatcher.read_status(goal).get('status')!='paused':gs['state']='ready'
   dispatcher.persist()
  while True:
   if queue:
    queue_api.boundary_updates(queue,a.manifest,state,Locks,audit=lambda proposed:Dispatcher(proposed,state).recover_and_audit())
    if queue_api.digest(dispatcher.m)!=queue_api.digest(queue_api.read(a.manifest)):
     dispatcher=Dispatcher(queue_api.read(a.manifest),state);dispatcher.pool_fds=locks.fds();dispatcher.recover_and_audit()
    queue_api.enroll(queue,dispatcher,a.manifest,registered_goal)
    queue_api.retry_goals(queue,dispatcher,inspect_job,Locks)
   if dispatcher.step():continue
   if not queue:
    if dispatcher.s['status'] in ('complete','paused'):break
    time.sleep(1);continue
   if a.exit_when_idle and dispatcher.s['status'] in ('complete','paused'):break
   # Only owner intake wakes idle admission. Existing provider backoff remains a scheduler timer.
   eligible=[g.get('next_at',time.time()) for g in dispatcher.s['goals'].values() if g['state'] not in ('complete','paused')]
   receiver.settimeout(max(.01,min(eligible)-time.time()) if eligible else None)
   try:receiver.recv(1)
   except TimeoutError:pass
 except KeyboardInterrupt:
  if 'dispatcher' in locals():dispatcher.s['status']='needs_attention';dispatcher.s['reason']='Owner stopped dispatcher; audit retained cycle cleanup before resuming';dispatcher.persist()
 finally:
  if receiver:receiver.close();queue.sock.unlink(missing_ok=True)
  if queue and queue_lock.files:save(queue.root/'owner.json',{'state':'stopped','process':None})
  locks.close();queue_lock.close()
if __name__=='__main__':main()
