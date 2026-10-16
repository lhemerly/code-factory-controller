"""Durable owner-approved admission intents for research-dispatch; never an acceptance ledger."""
import base64, copy, fcntl, hashlib, json, os, pathlib, socket, subprocess

def digest(v):return hashlib.sha256(json.dumps(v,sort_keys=True).encode()).hexdigest()
def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def read(p):return json.loads(pathlib.Path(p).read_text())
def pins(files):
 for path,expected in files.items():
  if sha(path)!=expected:raise ValueError('Queue source pin mismatch: '+path)

def write_bytes(destination,data):
 destination=pathlib.Path(destination);temp=destination.with_name(destination.name+'.queue-tmp')
 with temp.open('wb') as stream:stream.write(data);stream.flush();os.fsync(stream.fileno())
 os.replace(temp,destination)
 descriptor=os.open(destination.parent,os.O_RDONLY)
 try:os.fsync(descriptor)
 finally:os.close(descriptor)

def unlink_durable(path):
 path=pathlib.Path(path);path.unlink(missing_ok=True);descriptor=os.open(path.parent,os.O_RDONLY)
 try:os.fsync(descriptor)
 finally:os.close(descriptor)

class Queue:
 def __init__(self,root,save):
  self.root=pathlib.Path(root).resolve();self.save=save
  self.root.mkdir(parents=True,exist_ok=True,mode=0o700)
  if self.root.stat().st_uid!=os.getuid() or self.root.stat().st_mode&0o077:raise ValueError('Queue must be owner-only (0700)')
  self.path=self.root/'intents.json';self.sock=self.root/'wake.sock'
  if len(str(self.sock).encode())>103:raise ValueError('Queue path too long for Unix wake socket')
 def transaction(self,operation):
  with (self.root/'intake.lock').open('a') as lock:
   fcntl.flock(lock,fcntl.LOCK_EX)
   doc=read(self.path) if self.path.exists() else {'version':1,'goals':[],'updates':[]}
   result=operation(doc);self.save(self.path,doc)
  return result
 def snapshot(self):return self.transaction(lambda doc:copy.deepcopy(doc))
 def notify(self):
  with socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM) as channel:
   channel.setblocking(False)
   try:channel.sendto(b'q',str(self.sock))
   except (FileNotFoundError,ConnectionRefusedError,BlockingIOError):pass # Durable intents survive a missing wake-up.
 def enqueue(self,envelope,manifest,lookup):
  pins(manifest['trusted_files'])
  validate_goal(envelope,manifest,lookup)
  item={'id':digest(envelope),'input':envelope}
  def add(doc):
   for prior in doc['goals']:
    if prior['input']['goal']['id']==envelope['goal']['id']:
     if prior!=item:raise ValueError('Goal already queued with different approved scope')
     return prior
   if any(g['id']==envelope['goal']['id'] for g in manifest['goals']):raise ValueError('Existing project cannot be re-enrolled as a new queue intent')
   doc['goals'].append(item);return item
  result=self.transaction(add);self.notify();return result
 def stage(self,plan):
  validate_update(plan)
  item={'id':digest(plan),'input':plan}
  def add(doc):
   if item not in doc['updates']:doc['updates'].append(item)
   return item
  result=self.transaction(add);self.notify();return result
 def receiver(self):
  self.sock.unlink(missing_ok=True)
  channel=socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM);channel.bind(str(self.sock));os.chmod(self.sock,0o600);return channel
 def describe(self,state):
  result=[];snapshot=self.snapshot();applied={r['id'] for r in state.get('queue_retries',[])}
  for item in snapshot['goals']:
   name=item['input']['goal']['id'];gs=state.get('goals',{}).get(name);current=state.get('current') or {}
   status='queued' if gs is None else 'blocked' if state.get('status')=='needs_attention' and current.get('goal')==name else 'running' if current.get('goal')==name else 'completed' if gs.get('state')=='complete' else 'awaiting_review' if gs.get('state')=='paused' and gs.get('native_status')=='accepted' else 'blocked' if gs.get('state')=='paused' else 'queued'
   if current.get('goal')!=name and any(r['goal']==name and r['id'] not in applied for r in snapshot.get('retries',[])):status='retry_queued'
   result.append({'id':name,'intent':item['id'],'state':status,'software_milestones':len(gs.get('milestones',[])) if gs else 0})
  return result

def validate_goal(envelope,manifest,lookup):
 if set(envelope)!={'goal','trusted_files'}:raise ValueError('Goal intent must contain goal and trusted_files')
 goal=envelope['goal'];root=pathlib.Path(goal['root']).resolve()
 if pathlib.Path(goal['repository']).resolve()!=(root/'repository').resolve():raise ValueError('Queued repository must match the native goal root')
 if any(root not in pathlib.Path(p).resolve().parents for p in envelope['trusted_files']):raise ValueError('Goal intake pins must stay within its approved root')
 if goal.get('admission_mode') not in ['continuous','one_cycle']:raise ValueError('Explicit goal admission mode required')
 # Only the already approved native runner/routing can be reused; enqueue cannot introduce an executor.
 template=manifest['goals'][0];expected=[str(root) if v==template['root'] else v for v in template['cycle_command']]
 if goal['cycle_command']!=expected:raise ValueError('Queued goal cannot change runner, routing or shared circuits')
 required=[root/p for p in ['goal.json','supervisor.json','task.txt','job-template.json','protected.json','controller.json']]
 if any(str(p) not in envelope['trusted_files'] for p in required):raise ValueError('Missing approved goal/configuration pin')
 pins(envelope['trusted_files'])
 cfg=read(root/'supervisor.json')
 keys=['native','native_sha256','controller','controller_sha256','plugin','plugin_commit','venv','path','ownership_lease_seconds','ownership_heartbeat_seconds','min_free_bytes','max_state_bytes','unavailable_backoff_seconds']
 references=[read(pathlib.Path(existing['root'])/'supervisor.json') for existing in manifest['goals']]
 if not any(all(cfg.get(key)==reference.get(key) for key in keys) for reference in references):raise ValueError('Queued goal must match one complete approved runtime policy')
 registration=read(root/'controller.json')
 if registration.get('policy',{}).get('free_only') is not True or registration.get('policy',{}).get('spend_ceiling')!=0 or registration.get('max_workers')!=1:raise ValueError('Queued goal must preserve free-only single-worker policy')
 approved=read(root/'goal.json');observed=lookup(manifest['controller'],root/'controller.json')
 if approved.get('id')!=goal['id'] or observed.get('goal')!=approved:raise ValueError('Queue requires the exact independently registered standing goal')
 if pathlib.Path(read(root/'controller.json')['repository']).resolve()!=pathlib.Path(goal['repository']).resolve():raise ValueError('Registered goal repository mismatch')

def validate_update(plan):
 if set(plan)!={'previous_manifest','manifest','files','verification'}:raise ValueError('Invalid boundary update fields')
 old,new=plan['previous_manifest'],plan['manifest']
 if {k:v for k,v in old.items() if k not in ['controller','trusted_files']}!={k:v for k,v in new.items() if k not in ['controller','trusted_files']}:raise ValueError('Boundary update cannot change goals, commands, slots, permissions or acceptance')
 if new.get('worker_slots')!=1:raise ValueError('Boundary update must preserve one worker slot')
 if len({f['destination'] for f in plan['files']})!=len(plan['files']):raise ValueError('Duplicate update destination')
 for entry in plan['files']:
  if new['trusted_files'].get(entry['destination'])!=entry['sha256'] or sha(entry['source'])!=entry['sha256']:raise ValueError('Boundary update source/destination pin mismatch')
 destinations={e['destination'] for e in plan['files']}
 for path,value in old['trusted_files'].items():
  if path not in destinations and new['trusted_files'].get(path)!=value:raise ValueError('Boundary update cannot replace unrelated trusted pins')
 if set(new['trusted_files'])-set(old['trusted_files'])-destinations:raise ValueError('New trusted pins require exact staged files')
 code_parents={pathlib.Path(g['cycle_command'][1]).resolve().parent for g in old['goals']}
 for entry in plan['files']:
  destination=pathlib.Path(entry['destination']).resolve()
  runtime_code=destination.parent in code_parents and destination.suffix in ['.py','.mjs']
  controller_binary=destination==pathlib.Path(new['controller']).resolve() and destination.parent==pathlib.Path(old['controller']).resolve().parent
  configuration=False
  for goal in old['goals']:
   root=pathlib.Path(goal['root']).resolve()
   if destination==root/'supervisor.json':
    before=read(destination);after=read(entry['source'])
    configuration=({k:v for k,v in before.items() if k not in ['controller','controller_sha256']}=={k:v for k,v in after.items() if k not in ['controller','controller_sha256']} and after['controller']==new['controller'] and after['controller_sha256']==new['trusted_files'].get(new['controller']))
   if destination==root/'approved-policy-migration.json':
    migration=read(entry['source']);configuration=(migration.get('kind')=='lifecycle-recovery-repair' and migration.get('prior_configuration')==read(root/'supervisor.json'))
  if not (runtime_code or controller_binary or configuration):raise ValueError('Boundary update cannot change goal permissions or unrelated files: '+str(destination))
 check=plan['verification'];pins({check['path']:check['sha256']});report=read(check['path'])
 if report.get('passed') is not True or report.get('permissions_unchanged') is not True or not report.get('tests') or any(t.get('passed') is not True for t in report['tests']):raise ValueError('Boundary compatibility and permission tests must pass')


def restore(queue,journal):
 for entry in journal['backups']:
  destination=pathlib.Path(entry['path'])
  if entry['bytes'] is None:unlink_durable(destination)
  else:
   data=base64.b64decode(entry['bytes']);write_bytes(destination,data)
 queue.save(pathlib.Path(journal['manifest_path']),journal['manifest'])
 queue.save(pathlib.Path(journal['state_path']),journal['state'])
 unlink_durable(queue.root/'update-journal.json')

def boundary_updates(queue,manifest_path,state_path,locks,crash=None,audit=None):
 journal_path=queue.root/'update-journal.json'
 if journal_path.exists():restore(queue,read(journal_path))
 for item in queue.snapshot()['updates']:
  plan=item['input'];state=read(state_path);old=read(manifest_path)
  if any(m.get('id')==item['id'] for m in state.get('implementation_migrations',[])):continue
  validate_update(plan)
  if old!=plan['previous_manifest'] or state.get('manifest_digest')!=digest(old) or state.get('current'):raise ValueError('Boundary update requires exact idle prior manifest/state')
  pins(old['trusted_files'])
  held=locks()
  try:
   for goal in old['goals']:
    held.acquire(pathlib.Path(goal['root'])/'supervisor.lock')
    status_path=pathlib.Path(goal['root'])/'status.json'
    if status_path.exists() and read(status_path).get('current'):raise ValueError('Boundary goal still has unresolved execution')
   backups=[{'path':e['destination'],'bytes':base64.b64encode(pathlib.Path(e['destination']).read_bytes()).decode() if pathlib.Path(e['destination']).exists() else None} for e in plan['files']]
   journal={'manifest_path':str(manifest_path),'state_path':str(state_path),'manifest':old,'state':copy.deepcopy(state),'backups':backups}
   queue.save(journal_path,journal)
   try:
    for index,entry in enumerate(plan['files']):
     destination=pathlib.Path(entry['destination']);data=pathlib.Path(entry['source']).read_bytes()
     if hashlib.sha256(data).hexdigest()!=entry['sha256']:raise ValueError('Staged bytes changed during promotion')
     write_bytes(destination,data)
     if crash:crash(index)
    pins(plan['manifest']['trusted_files'])
    queue.save(manifest_path,plan['manifest'])
    state['manifest_digest']=digest(plan['manifest']);state.setdefault('implementation_migrations',[]).append({'kind':'boundary-update','id':item['id'],'from':digest(old),'to':state['manifest_digest']});queue.save(state_path,state)
    if audit:audit(plan['manifest'])
    unlink_durable(journal_path)
   except Exception:restore(queue,journal);raise
  finally:held.close()


def enroll(queue,dispatcher,manifest_path,lookup):
 pins(dispatcher.m['trusted_files'])
 if dispatcher.s.get('current'):raise RuntimeError('Cannot enroll during active execution')
 journal_path=queue.root/'enroll-journal.json'
 if journal_path.exists():
  journal=read(journal_path);queue.save(manifest_path,journal['manifest']);queue.save(dispatcher.path,journal['state']);unlink_durable(journal_path)
  dispatcher.m=journal['manifest'];dispatcher.s=journal['state']
 for item in queue.snapshot()['goals']:
  envelope=item['input'];goal=envelope['goal'];name=goal['id']
  if name in dispatcher.s['goals']:
   if dispatcher.s['goals'][name].get('queue_intent')!=item['id']:raise ValueError('Queued goal conflicts with an existing project')
   continue
  validate_goal(envelope,dispatcher.m,lookup)
  proposed=copy.deepcopy(dispatcher.m);proposed['goals'].append(goal)
  for path,value in envelope['trusted_files'].items():
   if path in proposed['trusted_files'] and proposed['trusted_files'][path]!=value:raise ValueError('Goal enrollment cannot replace existing trusted pins')
   proposed['trusted_files'][path]=value
  # Reuse the existing dispatcher constructor's root/overlap and ownership policy validation,
  # before changing any persisted manifest or state.
  for old in dispatcher.m['goals']:
   for a in [pathlib.Path(old['root']).resolve(),pathlib.Path(old['repository']).resolve()]:
    for b in [pathlib.Path(goal['root']).resolve(),pathlib.Path(goal['repository']).resolve()]:
     if a==b or a in b.parents or b in a.parents:raise ValueError('Queued goal scope overlaps an existing project')
  if pathlib.Path(goal['repository']).resolve()==pathlib.Path(goal['root']).resolve():raise ValueError('Goal repository must be a distinct worktree root')
  state=copy.deepcopy(dispatcher.s);state['goals'][name]={'state':'ready','milestones':[],'queue_intent':item['id']};state['cursor']=len(proposed['goals'])-1
  if any(gs.get('queue_intent') and gs['state'] not in ['paused','complete'] for gs in dispatcher.s['goals'].values()):state['cursor']=dispatcher.s['cursor']
  state['manifest_digest']=digest(proposed);state.setdefault('queue_enrollments',[]).append({'intent':item['id'],'goal':name,'previous_manifest':digest(dispatcher.m),'manifest':digest(proposed)})
  queue.save(journal_path,{'manifest':proposed,'state':state})
  queue.save(manifest_path,proposed);queue.save(dispatcher.path,state);unlink_durable(journal_path)
  dispatcher.m=proposed;dispatcher.s=state


def failed_input(manifest,goal,inspect):
 """Fresh controller-backed stop/candidate evidence; never infer acceptance from saved prose."""
 root=pathlib.Path(goal['root']);status=read(root/'status.json')
 if status.get('current'):raise RuntimeError('Goal still has active or uncertain execution')
 record_path=pathlib.Path(status['last_cycle']['result']);record=read(record_path)
 if record.get('goal_id')!=goal['id'] or record.get('sequence')!=status['sequence'] or record.get('status')!='failed':raise ValueError('Retry requires the exact last failed cycle')
 recovery=record.get('recovery') or {}
 if record.get('cleanup_confirmed') is not True or record.get('fence_error') or recovery.get('execution_stop',{}).get('status')!='confirmed' or recovery.get('accounting',{}).get('status')=='violating':raise ValueError('Retry requires confirmed cleanup/stop and no accounting violation')
 observed=inspect(manifest['controller'],record_path.parent/'controller.json',record['job_id']);current=observed.get('execution_recovery') or {}
 if current.get('stop_id')!=recovery.get('stop_id') or current.get('execution_stop',{}).get('status')!='confirmed' or current.get('accounting',{}).get('status')=='violating':raise ValueError('Controller did not confirm the exact owned stop')
 receipt=observed.get('active_verification_receipt') or {};candidate=record.get('candidate')
 if record.get('failure_kind')=='blocked' and not (candidate and receipt.get('outcome')=='failed'):raise ValueError('Permission/ownership blocker is not an automatic test repair')
 if candidate:
  saved=(record.get('candidate_status') or {}).get('active_verification_receipt') or {}
  if record.get('scope_approved') is not True or observed.get('candidate_commit')!=candidate or receipt.get('candidate_commit')!=candidate or receipt.get('verification_id')!=saved.get('verification_id') or receipt.get('outcome')!='failed':raise ValueError('Failed candidate receipt mismatch')
  cfg=read(root/'supervisor.json');template=read(root/'job-template.json');repo=goal['repository']
  exact=subprocess.check_output(['/usr/bin/git','-C',repo,'rev-parse',candidate+'^{commit}'],text=True).strip()
  if exact!=candidate:raise ValueError('Repair candidate must be exact')
  changed=subprocess.check_output(['/usr/bin/git','-C',repo,'diff','--name-only',cfg['initial_base'],candidate],text=True).splitlines()
  under=lambda p,r:p==r or p.startswith(r+'/')
  if any(not any(under(p,r) for r in template['allowed_paths']) or any(under(p,r) for r in template['protected_paths']) for p in changed):raise ValueError('Repair candidate exceeds the unchanged approved scope')
 return status,record,{'result_sha256':sha(record_path),'sequence':record['sequence'],'stop_id':current['stop_id'],'candidate':candidate,'receipt':receipt.get('verification_id')}


def request_retry(queue,manifest,goal_id,inspect,locks):
 pins(manifest['trusted_files']);goal=next(g for g in manifest['goals'] if g['id']==goal_id);held=locks()
 try:
  held.acquire(pathlib.Path(goal['root'])/'supervisor.lock');status,record,evidence=failed_input(manifest,goal,inspect)
  if (pathlib.Path(goal['root'])/'PAUSE').exists():raise ValueError('Explicit goal PAUSE must be reviewed before retry')
  item={'goal':goal_id,'evidence':evidence};item['id']=digest(item)
  def add(doc):
   retries=doc.setdefault('retries',[])
   if item not in retries:retries.append(item)
   return item
  result=queue.transaction(add);queue.notify();return result
 finally:held.close()


def retry_goals(queue,dispatcher,inspect,locks):
 if dispatcher.s.get('current'):raise RuntimeError('Retry cannot race active execution')
 for item in queue.snapshot().get('retries',[]):
  if any(r['id']==item['id'] for r in dispatcher.s.get('queue_retries',[])):continue
  goal=next(g for g in dispatcher.m['goals'] if g['id']==item['goal']);root=pathlib.Path(goal['root']);held=locks()
  try:
   held.acquire(root/'supervisor.lock');pins(dispatcher.m['trusted_files']);status,record,evidence=failed_input(dispatcher.m,goal,inspect)
   if evidence!=item['evidence'] or (root/'PAUSE').exists():raise ValueError('Retry evidence or pause changed since approval')
   if evidence['candidate']:status['base']=evidence['candidate']
   status['status']='waiting';status['pause_reason']=None;status['repair_resume']={'request':item['id'],'unaccepted_candidate':evidence['candidate'],'source_cycle':evidence['sequence'],'receipt':evidence['receipt']};queue.save(root/'status.json',status)
   gs=dispatcher.s['goals'][goal['id']];gs.update(state='ready',reason=None);dispatcher.s['cursor']=dispatcher.m['goals'].index(goal);dispatcher.s.setdefault('queue_retries',[]).append(item);dispatcher.persist()
  finally:held.close()
