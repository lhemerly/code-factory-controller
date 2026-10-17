#!/usr/bin/env python3
"""Finite owner-side publication of accepted Factory deliveries. No scheduler/model calls."""
import argparse,contextlib,fcntl,hashlib,importlib.util,json,os,pathlib,re,subprocess
ROOT=pathlib.Path(__file__).resolve().parent
TARGETS={'lhemerly/code-factory-controller':1414818646,'lhemerly/spark-scroll-mapper':1418813893,'lhemerly/ink-detector':1422424015}
def read(p):return json.loads(pathlib.Path(p).read_text())
def load_dispatch():
 spec=importlib.util.spec_from_file_location('main_dispatch',ROOT/'research-dispatch.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m

def run(args,**kw):return subprocess.run(args,check=True,capture_output=True,timeout=60,**kw)
def git(repo,*args):return run(['/usr/bin/git','-c','core.hooksPath=/dev/null','-C',str(repo),*args]).stdout.decode().strip()
def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def identity(b,s):
 if not re.fullmatch('[0-9a-f]{40}',b['initial_head']):raise ValueError('Exact bound initial main head required')
 if b['repository'] not in TARGETS or b['repository_id']!=TARGETS[b['repository']] or b['branch']!='main' or b['private'] is not True:raise ValueError('Destination is outside explicitly approved private main targets')
 if b['remote_url']!=f"git@github.com:{b['repository']}.git":raise ValueError('Exact canonical remote required')
 r=s['repository']
 if r['id']!=b['repository_id'] or r['full_name']!=b['repository'] or r['private']!=b['private'] or r['default_branch']!='main':raise ValueError('Remote identity, visibility or default branch changed')
 if not re.fullmatch('[0-9a-f]{40}',s['head']):raise ValueError('Exact remote head missing')
 if b.get('preflight_branch_prefix') is not None:
  if not re.fullmatch(r'factory/[A-Za-z0-9_-]{1,80}',b['preflight_branch_prefix']):raise ValueError('Preflight branch must stay in the approved factory namespace')
  if not isinstance(b.get('required_checks'),list) or not b['required_checks'] or not all(isinstance(x,str) and x for x in b['required_checks']):raise ValueError('Preflight requires explicit CI check names')

def ci_state(b,checks):
 # Exact-head GitHub API observations. Classify the observed inventory once so
 # every consumer agrees: unfinished or missing required checks are an expected
 # wait; failed/skipped required checks are a fence; no checks and no required
 # policy is not_configured, never an invented pass.
 required=b.get('required_checks')
 if not isinstance(required,list) or not all(isinstance(x,str) and x for x in required):raise ValueError('Explicit required_checks policy missing')
 if len(checks)>100:raise ValueError('CI inventory exceeds bound')
 failed=any(c.get('status')=='completed' and c.get('conclusion') not in ['success','neutral','skipped'] for c in checks)
 failed=failed or any(c.get('name') in required and c.get('status')=='completed' and c.get('conclusion')!='success' for c in checks)
 pending=any(c.get('status')!='completed' for c in checks) or any(not any(c['name']==name and c.get('conclusion')=='success' for c in checks) for name in required)
 state='failed' if failed else 'pending' if pending else 'passed' if checks else 'not_configured'
 return state,{'status':state,'checks':checks}

def checked_ci(b,s):
 # Strict exact-head gate before any main push. Missing, unfinished or failed
 # required checks must fence the push; only a green inventory may proceed.
 checks=s['checks'];required=b['required_checks']
 if not isinstance(required,list) or not all(isinstance(x,str) and x for x in required):raise ValueError('Explicit required_checks policy missing')
 if len(checks)>100:raise ValueError('CI inventory exceeds bound')
 if any(c.get('status')!='completed' or c.get('conclusion') not in ['success','neutral','skipped'] for c in checks):raise ValueError('Remote CI pending or failed')
 for name in required:
  matches=[c for c in checks if c['name']==name]
  if not matches or any(c.get('conclusion')!='success' for c in matches):raise ValueError('Required exact-head CI not passed: '+name)
 return {'status':'passed' if checks else 'not_configured','checks':checks}

class GitHub:
 def api(self,path):return json.loads(run(['gh','api','--hostname','github.com',path]).stdout)
 def snapshot(self,b,commit=None):
  r=self.api('repos/'+b['repository']);head=self.api('repos/'+b['repository']+'/git/ref/heads/main')['object']['sha'];target=commit or head
  c=self.api(f"repos/{b['repository']}/commits/{target}/check-runs?per_page=100&filter=all")
  suites_resp=self.api(f"repos/{b['repository']}/commits/{target}/check-suites?per_page=100")
  statuses=self.api(f"repos/{b['repository']}/commits/{target}/status?per_page=100")
  if c['total_count']>100 or suites_resp['total_count']>100 or statuses['total_count']>100:raise ValueError('CI inventory incomplete')
  # Run IDs order creation: queued reruns may have no start/completion timestamp.
  latest_checks={}
  for row in c['check_runs']:
   if row.get('head_sha')!=target:continue
   if not isinstance(row.get('id'),int):raise ValueError('CI run identity missing')
   if row['name'] not in latest_checks or row['id']>latest_checks[row['name']]['id']:latest_checks[row['name']]=row
  checks=[{'name':x['name'],'status':x['status'],'conclusion':x.get('conclusion')} for x in latest_checks.values()]
  checks += [{'name':x['context'],'status':'completed' if x['state']!='pending' else 'pending','conclusion':'success' if x['state']=='success' else 'failure'} for x in statuses['statuses']]
  suite_by_id={s.get('id'):s for s in suites_resp.get('check_suites',[])}
  branch=b.get('branch') or 'main'
  latest={}
  for run in c['check_runs']:
   suite=suite_by_id.get((run.get('check_suite') or {}).get('id'))
   if not isinstance(suite,dict):continue
   if suite.get('head_branch')!=branch:continue
   if suite.get('head_sha')!=target:continue
   if run.get('head_sha')!=target:continue
   name=run.get('name')
   key=run['id']
   if name not in latest or key>latest[name][0]:
    latest[name]=(key,run)
  main_checks=[{'name':run['name'],'status':run['status'],'conclusion':run.get('conclusion')} for _,run in latest.values()]
  return {'repository':r,'head':head,'checks':checks,'main_checks':main_checks,'checked_commit':target}
 def push(self,b,repo,commit):
  # Ordinary fast-forward ref update: concurrent divergent changes are rejected by Git.
  run(['/usr/bin/git','-c','core.hooksPath=/dev/null','-C',str(repo),'push',b['remote_url'],commit+':refs/heads/main'])
 def preflight_head(self,b,branch):
  ref='refs/heads/'+branch
  lines=run(['/usr/bin/git','ls-remote','--heads',b['remote_url'],ref]).stdout.decode().splitlines()
  if not lines:return None
  parts=lines[0].split()
  if len(lines)!=1 or len(parts)!=2 or parts[1]!=ref or not re.fullmatch('[0-9a-f]{40}',parts[0]):raise ValueError('Unexpected preflight ref observation')
  return parts[0]
 def push_preflight(self,b,repo,commit,branch):
  run(['/usr/bin/git','-c','core.hooksPath=/dev/null','-C',str(repo),'push',b['remote_url'],commit+':refs/heads/'+branch])

class Publisher:
 def __init__(self,state,remote=None):
  self.root=pathlib.Path(state).resolve();self.root.mkdir(parents=True,exist_ok=True,mode=0o700);self.remote=remote or GitHub();self.D=load_dispatch()
  if self.root.stat().st_uid!=os.getuid() or self.root.stat().st_mode&0o077:raise ValueError('Publication state must be owner-only')
 @contextlib.contextmanager
 def locked(self):
  with (self.root/'owner.lock').open('a') as f:fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);yield
 def save(self,s):self.D.save(self.root/'state.json',s)
 def await_ci(self,commit,receipt,ci):
  # An expected post-main CI wait is not a failure: retain durable intent and let
  # the existing dispatcher retry this same publication later.
  return {'state':'awaiting_ci','commit':commit,'receipt':receipt,'ci':ci}
 def verified(self,b,goal):
  if goal['id']!=b['goal_id'] or str(pathlib.Path(goal['root']).resolve())!=b['goal_root']:raise ValueError('Exact approved goal binding required')
  root=pathlib.Path(b['goal_root']);status=read(root/'status.json')
  if status.get('current'):raise ValueError('Cannot publish active execution')
  path=pathlib.Path(status['last_cycle']['result']);out=read(path);out['_result_path']=str(path)
  if out.get('status')!='accepted':raise ValueError('Only accepted delivery may publish')
  cfg=read(root/'supervisor.json')
  if sha(cfg['controller'])!=cfg['controller_sha256']:raise ValueError('Controller bytes changed')
  d=object.__new__(self.D.Dispatcher);d.m={'controller':cfg['controller']};milestone=d.audit_cycle(goal,out)
  if milestone is None:raise ValueError('Independent verification missing')
  repo=pathlib.Path(goal['repository']);commit=milestone['commit']
  if not re.fullmatch('[0-9a-f]{40}',commit):raise ValueError('Exact verified commit required')
  if repo.resolve()!=(root/'repository').resolve():raise ValueError('Exact goal repository required')
  # The same verified commit must advance main. No squash, rebase or untested merge here.
  git(repo,'merge-base','--is-ancestor',b['initial_head'],commit)
  return repo,commit,{**milestone,'result_sha256':sha(path)}
 def content(self,b,repo,previous,commit):
  paths=git(repo,'diff','--name-only',previous,commit).splitlines()
  if not paths:raise ValueError('No product changes to publish')
  if not b['allowed_paths'] or not b['forbidden_patterns']:raise ValueError('Explicit publication scope and content policy required')
  for p in paths:
   if not any(p==x or p.startswith(x+'/') for x in b['allowed_paths']) or any(p==x or p.startswith(x+'/') for x in b['protected_paths']):raise ValueError('Publication scope violation: '+p)
   if any(x in ['.git','.opencode','.aws','.ssh','node_modules','cycles'] for x in pathlib.PurePosixPath(p).parts) or pathlib.PurePosixPath(p).name.startswith('.env') or p.endswith(('.sqlite','.db','.pem','.key')):raise ValueError('Private runtime/credential artifact refused')
  # Scan the patch as data. No fetched or model-written code is executed by publication.
  patch=run(['/usr/bin/git','-C',str(repo),'diff','--no-ext-diff','--no-textconv',previous,commit,'--',*paths]).stdout
  # Git emits these markers only as column-0 header lines of a real binary diff;
  # every text-hunk content line carries a +/-/space prefix, so source literals
  # naming the markers are admitted text, never a binary header.
  binary_headers=(b'Binary files ',b'GIT binary patch')
  if len(patch)>2000000 or any(line.startswith(h) for line in patch.splitlines() for h in binary_headers):raise ValueError('Publication diff exceeds supported text scope')
  text=patch.decode()
  for pattern in [r'-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----',r'gh[pousr]_[A-Za-z0-9]{30,}',*b['forbidden_patterns']]:
   if re.search(pattern,text,re.I):raise ValueError('Publication content policy rejected patch')
  return hashlib.sha256(patch).hexdigest()
 def preflight(self,b,goal,repo,commit,receipt):
  observed=self.remote.snapshot(b);identity(b,observed)
  git(repo,'merge-base','--is-ancestor',observed['head'],commit)
  diff=self.content(b,repo,observed['head'],commit)
  path=self.root/'preflight.json';old=read(path) if path.exists() else None
  branch=b['preflight_branch_prefix']+'-'+commit
  if old and old['binding']!=b:raise ValueError('Immutable preflight binding changed')
  if old and old['commit']!=commit and old['state'] not in ['ready','ci_failed']:raise ValueError('Unresolved preflight owns this goal')
  current=self.remote.preflight_head(b,branch)
  if current is not None and current!=commit:raise ValueError('Preflight branch changed; no overwrite')
  record=old if old and old['commit']==commit else {'binding':b,'commit':commit,'branch':branch,'receipt':receipt,'diff_sha256':diff,'state':'push_intent'}
  if record['receipt']!=receipt or record['diff_sha256']!=diff:raise ValueError('Preflight receipt/content changed')
  if current is None:
   if old and old['commit']==commit:raise ValueError('Uncertain preflight not observed; no blind retry')
   self.D.save(path,record)
   try:self.remote.push_preflight(b,repo,commit,branch)
   except Exception:record['state']='uncertain';self.D.save(path,record);raise
   current=self.remote.preflight_head(b,branch)
   if current!=commit:record['state']='uncertain';self.D.save(path,record);raise ValueError('Preflight push not confirmed')
  elif not old or old['commit']!=commit:
   # An existing exact ref is observation, never proof of a prior local push.
   # Retain the audited receipt before CI/main effects under this binding.
   self.D.save(path,record)
  candidate=self.remote.snapshot(b,commit);identity(b,candidate)
  if candidate.get('checked_commit')!=commit:raise ValueError('Preflight CI belongs to another commit')
  state,_=ci_state(b,candidate['checks'])
  if state=='failed':record['state']='ci_failed';self.D.save(path,record);raise ValueError('Preflight CI failed')
  record.update(state='awaiting_ci' if state=='pending' else 'ready',ci={'commit':commit,'checks':candidate['checks']});self.D.save(path,record)
  return record
 def publish(self,b,goal):
  repo,commit,receipt=self.verified(b,goal)
  if b.get('require_goal_completion'):
   evaluation=read(pathlib.Path(b['goal_root'])/'goal-evaluation.json');result=evaluation.get('result',{})
   if evaluation.get('goal_id')!=goal['id'] or evaluation.get('commit')!=commit or result.get('commit')!=commit or result.get('passed') is not True or result.get('status')!='passed' or not result.get('receipts'):raise ValueError('Independent whole-goal completion missing')
   self.D.acceptance.audit_result({**goal['acceptance'],'repository':goal['repository'],'verifier_executable':self.D.queue_api.read(pathlib.Path(b['goal_root'])/'supervisor.json')['controller']},result,commit)
  existing=read(self.root/'state.json') if (self.root/'state.json').exists() else None
  if existing and existing['binding']!=b:raise ValueError('Immutable publication binding changed')
  if existing and existing.get('intent',{}).get('state')!='confirmed' and existing.get('intent',{}).get('commit')!=commit:raise ValueError('Unresolved main intent owns this goal')
  if b.get('preflight_branch_prefix') and (not existing or existing.get('intent',{}).get('commit')!=commit):
   preflight=self.preflight(b,goal,repo,commit,receipt)
   if preflight['state']!='ready':return {'state':'awaiting_ci','commit':commit,'branch':preflight['branch'],'receipt':receipt,'ci':preflight['ci']}
  observed=self.remote.snapshot(b,commit);identity(b,observed)
  if observed.get('checked_commit')!=commit:raise ValueError('CI observation belongs to another commit')
  old=read(self.root/'state.json') if (self.root/'state.json').exists() else None
  if old and old['binding']!=b:raise ValueError('Immutable publication binding changed')
  intent=(old or {}).get('intent')
  if intent and intent['state']!='confirmed':
   if intent['commit']!=commit or intent['receipt']!=receipt:raise ValueError('Unresolved publication owns this goal; preserve prior receipt')
   previous=intent['previous_head']
   if observed['head']!=commit:raise ValueError('Uncertain push not confirmed; no blind retry')
  else:
   if intent and intent['commit']==commit:
    if observed['head']!=commit:raise ValueError('Published main changed; reconcile before new effects')
    post_checks=observed.get('main_checks',[]) if b.get('preflight_branch_prefix') else observed.get('checks',[])
    state,ci=ci_state(b,post_checks)
    if state=='failed':raise ValueError('Remote CI failed')
    if state=='pending':return self.await_ci(commit,receipt,ci)
    return {**intent,'ci':ci}
   previous=observed['head']
   git(repo,'merge-base','--is-ancestor',previous,commit)
  diff=self.content(b,repo,previous,commit)
  if observed['head']!=commit:
   # Strict pre-push gate: observed CI on the current main must be green before
   # any main effect. Missing/unfinished/failed checks fence here with no push.
   ci=checked_ci(b,observed)
   intent={'state':'push_intent','commit':commit,'previous_head':previous,'receipt':receipt,'diff_sha256':diff,'ci_before':ci};s={'binding':b,'history':(old or {}).get('history',[]),'intent':intent};self.save(s)
   try:self.remote.push(b,repo,commit)
   except Exception:intent['state']='uncertain';self.save(s);raise
  else:
   if not old or not intent:raise ValueError('Remote commit lacks retained publication intent')
   s=old
  after=self.remote.snapshot(b,commit);identity(b,after)
  if after['head']!=commit or after.get('checked_commit')!=commit:raise ValueError('Main update not observed; intent retained')
  after_checks=after.get('main_checks',[]) if b.get('preflight_branch_prefix') else after.get('checks',[])
  intent.update(state='remote_observed',ci_observation={'commit':commit,'checks':after_checks});s['intent']=intent;self.save(s)
  # Post-main CI is an expected wait after observing the exact retained intent;
  # failed checks stay fenced and never become acceptance.
  state,ci=ci_state(b,after_checks)
  if state=='failed':raise ValueError('Remote CI failed')
  if state=='pending':return self.await_ci(commit,receipt,ci)
  intent.update(state='confirmed',ci=ci);s['history'].append(dict(intent));self.save(s);return intent

def main():
 p=argparse.ArgumentParser();p.add_argument('--state',required=True);p.add_argument('--binding',required=True);p.add_argument('--goal',required=True);a=p.parse_args();publisher=Publisher(a.state)
 with publisher.locked():print(json.dumps(publisher.publish(read(a.binding),read(a.goal)),indent=2))
if __name__=='__main__':main()
