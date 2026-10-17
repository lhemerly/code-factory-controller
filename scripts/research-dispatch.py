#!/usr/bin/env python3
"""Foreground, one-slot operator for existing Code Factory goal-cycle runners.
Native execution and receipts remain authoritative in the existing runtime/controller.
"""
import argparse, datetime, fcntl, hashlib, importlib.util, json, math, os, pathlib, shutil, signal, subprocess, sys, time, uuid
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

def audited_recovery_usage(out, recovery):
 """Report one freshly inspected session total; preserve unknown original usage otherwise."""
 original=out.get('usage');session=out.get('child') or out.get('session')
 if (out.get('status')!='failed' or not session or not isinstance(recovery,dict)
     or recovery.get('session_id')!=session or recovery.get('attempt_id')!=out.get('attempt')
     or not out.get('attempt') or not isinstance(recovery.get('stop_id'),str) or not recovery['stop_id']
     or recovery.get('execution_stop',{}).get('status')!='confirmed'):
  return original
 accounting=recovery.get('accounting') or {}
 if accounting.get('status')!='settled_zero':return original
 observations=accounting.get('observations')
 if not isinstance(observations,list):return original
 sessions=[row for row in observations if isinstance(row,dict) and row.get('source')=='session']
 if len(sessions)!=1 or sessions[0].get('id')!=session:return original
 row=sessions[0];tokens=row.get('tokens');cost=row.get('cost')
 if type(cost) not in (int,float) or cost!=0 or not isinstance(tokens,dict):return original
 cache=tokens.get('cache')
 if not isinstance(cache,dict):return original
 values=[tokens.get(k) for k in ('input','output','reasoning')]+[cache.get(k) for k in ('read','write')]
 if any(type(v) not in (int,float) or not math.isfinite(v) or v<0 for v in values):return original
 return {'cost':cost,'tokens':{'input':values[0],'output':values[1],'reasoning':values[2],
          'cache':{'read':values[3],'write':values[4]}},'model':row.get('model'),
         'accounting_status':'settled_zero','source':'controller_recovery_session',
         'session_id':session,'stop_id':recovery.get('stop_id')}

def merge_circuits(*sources):
 result={}
 for source in sources:
  for model,circuit in source.items():
   if model not in result or circuit['until']>result[model]['until']:result[model]=circuit
 return result

def migrate_launch_policy(manifest,state_path,migration_path):
 """Explicit owner repair of pinned implementation; goal/permission contracts stay exact."""
 state_path=pathlib.Path(state_path);state=json.loads(state_path.read_text());migration=json.loads(pathlib.Path(migration_path).read_text());previous=migration['previous_manifest']
 kind=migration.get('kind');publication=kind=='verified-main-publication'
 if kind not in ['lifecycle-recovery-repair','verified-main-publication'] or migration.get('from')!=state['manifest_digest'] or digest(previous)!=migration['from'] or digest(manifest)!=migration.get('to'):raise ValueError('Repair migration identity mismatch')
 allowed=['controller','trusted_files']+(['main_publication'] if publication else [])
 if {k:v for k,v in previous.items() if k not in allowed}!={k:v for k,v in manifest.items() if k not in allowed}:raise ValueError('Repair cannot change goals, scope, commands, pricing or acceptance contracts')
 if publication:
  policies=manifest.get('main_publication',{});prior=previous.get('main_publication',{});goals={g['id']:g for g in manifest['goals']}
  if not policies or any(k not in goals for k in policies) or any(policies.get(k)!=v for k,v in prior.items()):raise ValueError('Publication migration cannot add goals or replace existing bindings')
  pub=module('main_policy','main-publication.py')
  for goal_id,entry in policies.items():
   goal=goals[goal_id];binding=queue_api.read(entry['binding']);template=queue_api.read(pathlib.Path(goal['root'])/'job-template.json')
   if binding['goal_id']!=goal_id or pathlib.Path(binding['goal_root']).resolve()!=pathlib.Path(goal['root']).resolve() or binding['allowed_paths']!=template['allowed_paths'] or binding['protected_paths']!=template['protected_paths']:raise ValueError('Publication binding changes approved goal scope')
   pub.identity(binding,{'repository':{'id':binding['repository_id'],'full_name':binding['repository'],'private':binding['private'],'default_branch':'main'},'head':binding['initial_head']})
   for source in [entry['binding'],str(ROOT/'main-publication.py')]:
    if manifest.get('trusted_files',{}).get(source)!=queue_api.sha(source):raise ValueError('Publication migration requires pinned binding/code')
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
  for goal in goals:
   base=goal.get('initial_base')
   if base is not None:
    if not isinstance(base,str) or len(base)!=40 or any(c not in '0123456789abcdef' for c in base):raise ValueError('Initial goal evaluation requires an exact commit')
    config=pathlib.Path(goal['root'])/'supervisor.json'
    if config.exists() and json.loads(config.read_text()).get('initial_base')!=base:raise ValueError('Initial goal evaluation disagrees with approved cycle base')
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
  self.s['circuits']=merge_circuits(self.s.get('circuits',{}),self.shared_circuits(),*(self.read_status(g).get('circuits',{}) for g in goals))
  self.persist()
 def persist(self):self.s['updated_utc']=utc();save(self.path,self.s)
 def shared_circuits(self):
  path=self.m.get('circuits_file')
  if not path:return {}
  try:return queue_api.read(path)
  except FileNotFoundError:return {}
 def sync_circuits(self,*sources):
  # Re-read immediately before writing: an owner update may arrive after startup.
  self.s['circuits']=merge_circuits(self.s.get('circuits',{}),self.shared_circuits(),*sources)
  if self.m.get('circuits_file'):save(pathlib.Path(self.m['circuits_file']),self.s['circuits'])
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
  if record.get('phase')=='publication':
   self.audit_completed();self.publication_tick(goal);self.s['cursor']=(record['index']+1)%len(self.m['goals']);self.s['current']=None;self.persist();return
  if record.get('phase')=='baseline_evaluation':
   if record.get('commit')!=goal.get('initial_base') or status.get('sequence',0)!=0 or status.get('current'):raise RuntimeError('Baseline evaluation recovery identity mismatch')
   if status.get('status')=='paused' or (pathlib.Path(goal['root'])/'PAUSE').exists():raise RuntimeError('Owner pause prevents baseline evaluation recovery')
   self.evaluate_baseline(goal);self.s['cursor']=(record['index']+1)%len(self.m['goals']);self.s['current']=None;self.persist();return
  if record.get('phase')=='native_recovery':
   if status.get('current') and status['current'].get('process') and same_process(status['current']['process']):raise RuntimeError('Existing cycle owner is alive; do not duplicate')
   acceptance.audit_processes(self.path.parent/(goal['id']+'-goal'))
   status=self.read_status(goal)
   self.finish(goal,status);self.s['cursor']=(record['index']+1)%len(self.m['goals']);self.s['current']=None;self.persist();return
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
   out['_audited_recovery']=current
   return None
  config=pathlib.Path(out['_result_path']).parent/'controller.json'
  p=subprocess.run([self.m['controller'],'inspect','--config',str(config),'--job',out['job_id']],capture_output=True,text=True,check=True)
  authoritative=json.loads(p.stdout);receipt=authoritative.get('active_verification_receipt') or {}
  if authoritative.get('state')!='done' or authoritative.get('outcome')!='verified_delivery' or receipt.get('outcome')!='passed' or authoritative.get('integrated_commit')!=out['acceptance']['integrated_commit'] or receipt.get('verification_id')!=out['acceptance']['active_verification_receipt']['verification_id']:raise RuntimeError('Controller did not confirm exact passing integrated delivery')
  delivery=authoritative['delivery'];archive=pathlib.Path(delivery['destination'])
  if hashlib.sha256(archive.read_bytes()).hexdigest()!=delivery['sha256']:raise RuntimeError('Delivered artifact digest mismatch')
  return {'job':out['job_id'],'commit':authoritative['integrated_commit'],'receipt':receipt['verification_id'],'result':out['_result_path']}
 def complete_goal(self,goal,milestone):
  manifest={**goal['acceptance'],'repository':goal['repository'],'verifier_executable':self.m['controller']}
  result=acceptance.run_acceptance(manifest,self.path.parent/(goal['id']+'-goal'),milestone['commit'])
  # The existing clean-checkout independent acceptance implementation supplies receipts.
  return result
 def publish_goal(self,goal):
  publication=self.m['main_publication'][goal['id']]
  for path in [publication['binding'],str(ROOT/'main-publication.py')]:
   if self.m.get('trusted_files',{}).get(path)!=queue_api.sha(path):raise ValueError('Main publication binding/code is not pinned')
  publisher=module('main_publication','main-publication.py').Publisher(publication['state'])
  with publisher.locked():return publisher.publish(queue_api.read(publication['binding']),goal)
 def publication_tick(self,goal):
  gs=self.s['goals'][goal['id']];gs['publication']=self.publish_goal(goal)
  if gs['publication']['state']=='confirmed':
   gs['state']=gs.pop('publication_next_state');gs.pop('reason',None)
  elif gs['publication']['state']=='awaiting_ci':gs['state']='publishing';gs['next_at']=self.clock()+30
  else:raise RuntimeError('Unrecognized publication state; preserve intent')
 def record_evaluation(self,goal,commit,result,milestone_receipt=None,baseline=False):
  self.s['goals'][goal['id']]['goal_acceptance']=result
  record={
   'version':1,'goal_id':goal['id'],'commit':commit,'baseline':baseline,
   'milestone_receipt':milestone_receipt,'evaluated_utc':utc(),'result':result}
  save(pathlib.Path(goal['root'])/'goal-evaluation.json',record)
  return record
 def evaluate_baseline(self,goal):
  gs=self.s['goals'][goal['id']];commit=goal['initial_base']
  result=self.complete(goal,{'commit':commit});record=self.record_evaluation(goal,commit,result,baseline=True)
  gs['baseline_evaluation']={'commit':commit,'finished':not result.get('execution_unresolved',False),'evidence':record}
  if result.get('execution_unresolved'):
   gs['state']='paused';gs['reason']='Baseline verifier execution unresolved; preserve process evidence';return
  if result.get('passed') is True and result.get('status')=='passed' and result.get('commit')==commit and result.get('receipts'):
   gs['state']='complete';gs['completion_base']=commit;return
  if result.get('status')=='awaiting_acceptance' or result.get('phase') in ('policy','setup','check_launch'):
   gs['state']='paused';gs['reason']='Approved goal verification prerequisites unavailable: '+result.get('reason','inspect retained evaluation');return
  gs['state']='ready';gs['next_at']=self.clock()
 def finish(self,goal,status):
  gs=self.s['goals'][goal['id']]
  if status.get('current'):gs['state']='paused';raise RuntimeError('Goal has unresolved active state; cannot reassign')
  last=status.get('last_cycle')
  if not last:
   gs['state']='paused';gs['reason']=status.get('pause_reason','No completed cycle');return
  p=pathlib.Path(last['result']);out=json.loads(p.read_text());out['_result_path']=str(p)
  if out.get('sequence')!=status['sequence'] or out.get('goal_id')!=goal['id']:raise RuntimeError('Cycle/goal identity mismatch')
  # Saved JSON cannot supply the private current-controller observation.
  out.pop('_audited_recovery',None)
  milestone=self.audit(goal,out)
  gs['last_result']=str(p);gs['usage']=audited_recovery_usage(out,out.get('_audited_recovery'));gs['model']=out.get('model');gs['native_status']=out.get('status');gs['harness']=goal['cycle_command']
  self.sync_circuits(status.get('circuits',{}))
  if milestone:
   if not any(m['job']==milestone['job'] for m in gs['milestones']):gs['milestones'].append(milestone)
   result=self.complete(goal,milestone)
   # Feed independent whole-goal failures back into the existing pstack admission.
   # Save before releasing this cycle: restart reconciles the same milestone,
   # never invents acceptance from the feedback or asks a human to plan repairs.
   self.record_evaluation(goal,milestone['commit'],result,milestone['receipt'])
   if result.get('execution_unresolved'):
    gs['state']='paused';gs['reason']='Independent goal verifier ownership/quiescence unconfirmed; preserve process evidence';return
   if result.get('status')=='awaiting_acceptance' or result.get('phase') in ('policy','setup','check_launch'):
    gs['state']='paused';gs['reason']='Approved goal verification prerequisites unavailable: '+result.get('reason','inspect retained evaluation');return
   passed=result.get('passed') is True and result.get('status')=='passed' and result.get('commit')==milestone['commit'] and bool(result.get('receipts'))
   publication=self.m.get('main_publication',{}).get(goal['id'])
   if publication:
    binding=queue_api.read(publication['binding'])
    if passed or not binding.get('require_goal_completion'):
     gs['publication']=self.publish_goal(goal)
     if gs['publication']['state']=='awaiting_ci':
      gs['state']='publishing';gs['next_at']=self.clock()+30
      gs['publication_next_state']='complete' if passed else 'paused' if goal.get('admission_mode')=='one_cycle' else 'ready'
      if passed:gs['completion_milestone']=milestone
      return
     if gs['publication']['state']!='confirmed':raise RuntimeError('Unrecognized publication state')
   if passed:
    gs['state']='complete';gs['completion_milestone']=milestone;gs.pop('reason',None);return
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
  if self.s['goals'][goal['id']]['state']=='publishing':
   self.s['current']={'goal':goal['id'],'index':index,'phase':'publication'};self.persist()
   try:self.audit_completed();self.publication_tick(goal)
   except Exception as error:
    self.s['status']='needs_attention';self.s['goals'][goal['id']].update(state='paused',reason=str(error));self.persist();raise
   self.s['current']=None;self.s['cursor']=(index+1)%len(self.m['goals']);self.persist();return True
  if goal.get('initial_base') and not self.s['goals'][goal['id']].get('baseline_evaluation'):
   if prior.get('status')=='paused' or (pathlib.Path(goal['root'])/'PAUSE').exists():
    self.s['goals'][goal['id']].update(state='paused',reason='Owner pause before baseline evaluation');self.persist();return True
   if prior.get('sequence',0)!=0 or prior.get('current'):raise RuntimeError('Initial evaluation requires a fresh goal, not an unresolved or existing cycle')
   self.s['status']='evaluating';self.s['current']={'goal':goal['id'],'index':index,'phase':'baseline_evaluation','commit':goal['initial_base']};self.persist()
   try:self.evaluate_baseline(goal)
   except Exception as error:
    self.s['status']='needs_attention';self.persist();raise
   self.s['current']=None;self.s['cursor']=(index+1)%len(self.m['goals']);self.persist();return True
  # Explicit one-cycle admissions may await an owner-defined completion contract;
  # they already stop after their bounded milestone. Continuous goals must have
  # usable checks before spending any coding work, even without initial_base.
  if goal.get('admission_mode')!='one_cycle':
   try:acceptance.validate_policy({**goal.get('acceptance',{}),'repository':goal['repository']})
   except ValueError as error:
    self.s['goals'][goal['id']].update(state='paused',reason='Approved goal verification prerequisites unavailable: '+str(error));self.persist();return True
  self.sync_circuits()
  self.s['status']='running';self.s['current']={'goal':goal['id'],'index':index,'expected_sequence':prior['sequence']+1,'phase':'spawning'};self.persist()
  try:
   self.runner(goal,self.s['current']);status=self.read_status(goal)
   if 'exit_status' in self.s['current']:
    self.s['goals'][goal['id']]['last_host_exit']={
     'sequence':self.s['current']['expected_sequence'],
     'exit_status':self.s['current']['exit_status'],'observed_utc':utc()}
   if status['sequence']!=self.s['current']['expected_sequence']:
    if status['sequence']==self.s['current']['expected_sequence']-1 and status.get('status')=='paused' and not status.get('current'):
     self.s['goals'][goal['id']].update(state='paused',reason=status.get('pause_reason','Paused before admission'))
    else:raise RuntimeError('Expected one new completed cycle')
   else:
    if not self.try_native_recovery(goal,status):
     self.finish(goal,status)
  except Exception as error:
   self.s['status']='needs_attention';self.s['goals'][goal['id']]['state']='paused';self.s['goals'][goal['id']]['reason']=str(error);self.persist();raise
  self.s['cursor']=(index+1)%len(self.m['goals']);self.s['current']=None;self.persist();return True
 def native_recovery_qualified(self,goal,status,out):
  if out.get('status')!='failed':return False
  if out.get('cleanup_confirmed') is not True:return False
  if out.get('phase') not in ('native','native_shells'):return False
  if out.get('failure_kind') not in ('transient','unavailable'):return False
  root=pathlib.Path(goal['root'])
  if (root/'PAUSE').exists():return False
  if status.get('pause_reason') and 'PAUSE' in str(status.get('pause_reason')):return False
  current=status.get('current') or {}
  if current.get('process') and same_process(current['process']):return False
  helper=ROOT/'reconcile-stopped-v2.mjs'
  trusted=self.m.get('trusted_files',{})
  if str(helper) not in trusted:return False
  try:
   if hashlib.sha256(helper.read_bytes()).hexdigest()!=trusted[str(helper)]:return False
  except Exception:return False
  return True
 def try_native_recovery(self,goal,status):
  last=status.get('last_cycle')
  if not last:return False
  p=pathlib.Path(last['result'])
  try:out=json.loads(p.read_text())
  except Exception:return False
  out['_result_path']=str(p)
  # A confirmed stop avoids another helper, but still requires finish() to
  # audit the exact controller receipt and honor the saved pause before failover.
  current=status.get('current')
  recovery=out.get('recovery') or {}
  already_stopped = (
   out.get('status')=='failed' and
   out.get('phase') in ('native','native_shells') and
   not current and
   out.get('cleanup_confirmed') is True and
   not out.get('fence_error') and
   recovery.get('execution_stop',{}).get('status')=='confirmed' and
   recovery.get('accounting',{}).get('status')!='violating')
  if already_stopped:return False
  if not self.native_recovery_qualified(goal,status,out):
   if out.get('status')=='failed' and out.get('phase') in ('native','native_shells'):
    raise RuntimeError('Native transport recovery fence: unqualified stopped result')
   return False
  helper=ROOT/'reconcile-stopped-v2.mjs'
  trusted=self.m.get('trusted_files',{})
  if str(helper) not in trusted or hashlib.sha256(helper.read_bytes()).hexdigest()!=trusted[str(helper)]:
   raise RuntimeError('Native recovery helper is not pinned')
  # Audit any prior owned helper plans before executing another one.
  state_path=self.path.parent/(goal['id']+'-goal')
  acceptance.audit_processes(state_path)
  # Persist current phase before helper execution.
  if not self.s['current']:return False
  self.s['current']['phase']='native_recovery'
  self.persist()
  outdir=pathlib.Path(str(state_path)+'.acceptance')/'native_recovery'
  outdir.mkdir(parents=True,exist_ok=True)
  stdout=outdir/(uuid.uuid4().hex+'.stdout')
  stderr=stdout.with_suffix('.stderr')
  receipt={}
  node=shutil.which('node') or '/usr/bin/node'
  argv=[node,str(helper),str(pathlib.Path(goal['root'])),str(self.m['controller'])]
  env={'PATH':os.environ.get('PATH','/usr/bin:/bin')}
  commit=out.get('base')
  if commit is None:commit=status.get('base')
  if commit is None:commit=goal.get('initial_base')
  if commit is not None:env['FACTORY_VERIFICATION_COMMIT']=commit
  v2=os.environ.get('FACTORY_OPENCODE_V2_CLIENT')
  if v2 is not None:env['FACTORY_OPENCODE_V2_CLIENT']=v2
  executable=self.m['controller']
  timeout=600
  failure=acceptance.supervised_check(executable,argv,pathlib.Path(goal['root']),env,timeout,stdout,stderr,receipt)
  if failure is not None:raise RuntimeError(str(failure))
  status=self.read_status(goal)
  self.finish(goal,status)
  return True
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
    record['exit_status']=code;self.persist()
    if code:
     # A host can fail after durably saving its terminal result. Apply exactly
     # the normal sequence/cleanup/controller audit in step(), rather than
     # requiring an operator restart to reach recover(). Exit status alone
     # never authorizes replacement or accepts a worker claim.
     status=self.read_status(goal)
     if (status.get('sequence')!=record['expected_sequence'] or status.get('current')
         or not status.get('last_cycle')):
      raise RuntimeError('Goal cycle exited '+str(code)+' without a completed result; inspect '+str(log))
  finally:locks.close()
 def audit_completed(self):
  for g in self.m['goals']:
   gs=self.s['goals'][g['id']]
   if gs.get('goal_acceptance',{}).get('execution_unresolved'):
    # Parent loss may have closed the helper's control pipe while cleanup was
    # still finishing. Re-evaluate the same accepted milestone only after the
    # existing supervisor records positive quiescence; never dispatch coding
    # work or clear an unknown execution on an elapsed-time assumption.
    if not acceptance.audit_processes(self.path.parent/(g['id']+'-goal')):
     raise RuntimeError('Independent goal verifier execution unresolved; inspect retained process evidence before resume')
    if gs.get('baseline_evaluation',{}).get('finished') is False:self.evaluate_baseline(g)
    else:self.finish(g,self.read_status(g))
    self.persist()
    if gs.get('goal_acceptance',{}).get('execution_unresolved'):
     raise RuntimeError('Independent goal verifier execution unresolved after evidence recheck')
   for milestone in gs.get('milestones',[]):
    out=json.loads(pathlib.Path(milestone['result']).read_text());out['_result_path']=milestone['result']
    if self.audit(g,out)!=milestone:raise RuntimeError('Stored milestone identity changed')
   if gs['state'] not in ['complete','publishing']:continue
   result=gs['goal_acceptance']
   if gs['state']=='complete' or result.get('passed') is True:
    milestone=gs.get('completion_milestone') or (gs.get('milestones') or [{}])[-1]
    commit=gs.get('completion_base') or milestone.get('commit')
    try:acceptance.audit_result({**g['acceptance'],'repository':g['repository'],'verifier_executable':self.m['controller']},result,commit)
    except (ValueError,KeyError,OSError,TypeError) as error:raise RuntimeError('Goal completion evidence changed or ownership unconfirmed: '+str(error)) from error
   else:
    # Publication may deliberately require only a verified milestone. Failed
    # whole-goal checks remain failures; do not apply a success-only audit.
    for receipt in result.get('receipts',[]):acceptance.audit_logs(receipt)

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
