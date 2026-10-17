"""Read-only audit of an owner-pinned contract-only permission-block resolution.
Creating/pinning the record requires independently verified human authorization.
This command neither grants access, retries the old job, nor changes accounting.
"""
import argparse,hashlib,json,os,pathlib,re,stat,subprocess
NAME='contract-only-resolution.json'
def digest(x):return hashlib.sha256(json.dumps(x,sort_keys=True).encode()).hexdigest()
def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def read(p):return json.loads(pathlib.Path(p).read_text())
def require(ok,why):
 if not ok:raise ValueError(why)
def regular(p):
 p=pathlib.Path(p);s=p.lstat();require(stat.S_ISREG(s.st_mode) and s.st_uid==os.getuid() and not s.st_mode&0o022,'Untrusted owner file: '+str(p));return p

# Corroborate target identity through an ancestor open FD, its kernel FLOCK
# record, and /proc/locks. Stat and lock devices may differ across mounts.
def _ancestry():
 result={};pid=os.getpid()
 for _ in range(64):
  try:
   fields=pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split();result[pid]=fields[19];parent=int(fields[1])
  except (FileNotFoundError,ValueError):break
  if parent<=1 or parent in result:break
  pid=parent
 return result

def _kernel_locks(text,fdinfo=False):
 result=[]
 for line in text.splitlines():
  fields=line.split()
  if fdinfo:
   if not fields or fields[0]!='lock:':continue
   fields=fields[1:]
  if len(fields)==8 and fields[1:4]==['FLOCK','ADVISORY','WRITE'] and fields[6:]==['0','EOF']:
   result.append(tuple(fields[1:]))
 return result

def owner_lock(path):
 path=pathlib.Path(path);target=path.lstat()
 if not stat.S_ISREG(target.st_mode):raise ValueError('Lock target is not a regular file')
 before=_ancestry();global_locks=_kernel_locks(pathlib.Path('/proc/locks').read_text())
 for pid,ticks in before.items():
  try:fds=list(pathlib.Path(f'/proc/{pid}/fd').iterdir())
  except (FileNotFoundError,PermissionError):continue
  for fd in fds:
   try:
    observed=fd.stat()
    if (observed.st_dev,observed.st_ino)!=(target.st_dev,target.st_ino):continue
    text=pathlib.Path(f'/proc/{pid}/fdinfo/{fd.name}').read_text();metadata=dict(line.split(':',1) for line in text.splitlines() if ':' in line and not line.startswith('lock:'))
    if int(metadata.get('ino','-1'))!=target.st_ino:continue
    for lock in _kernel_locks(text,True):
     if lock[3]!=str(pid) or lock not in global_locks:continue
     current=pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19]
     latest=path.lstat()
     if current!=ticks or (latest.st_dev,latest.st_ino)!=(target.st_dev,target.st_ino):continue
     if lock not in _kernel_locks(pathlib.Path('/proc/locks').read_text()):continue
     return {'pid':pid,'fd':fd.name,'stat_device':f'{os.major(target.st_dev):02x}:{os.minor(target.st_dev):02x}','kernel_lock_key':lock[4],'inode':target.st_ino,'start_ticks':ticks}
   except (FileNotFoundError,PermissionError,ValueError):continue
 raise ValueError('Exclusive owner lock unavailable: '+str(path))

def audit(root,directory,controller,next_sequence):
 root=pathlib.Path(root).resolve();directory=pathlib.Path(directory).resolve();proof=regular(root/NAME);v=read(proof)
 require(v.get('version')==1 and v.get('kind') in ['contract-only-checker-reference','owner-checkout-isolation','host-git-ownership'],'Unsupported resolution')
 host_git=v['kind']=='host-git-ownership'
 require(directory.parent==root/'cycles' and directory.name.isdigit(),'Foreign cycle')
 original=regular(directory/'result.json');out=read(original);sequence=int(directory.name)
 require(v['original_sha256']==sha(original) and v['sequence']==sequence==out['sequence'] and next_sequence==sequence+1,'Stale failure or next sequence')
 require(out['status']=='failed' and out.get('failure_kind')=='blocked' and out.get('phase')=='native' and out.get('cleanup_confirmed') is True and not out.get('fence_error'),'Not a confirmed failed native block')
 require(not out.get('candidate') and not out.get('acceptance'),'Resolution cannot approve candidate work')
 identity={k:out.get(k) for k in ['goal_id','job_id','attempt','child','parent','worktree']};require(v['identity']==identity and all(isinstance(x,str) and x for x in identity.values()),'Execution identity mismatch')
 cfg=read(root/'supervisor.json');goal=read(root/'goal.json');require(goal['id']==out['goal_id'] and pathlib.Path(cfg['controller']).resolve()==pathlib.Path(controller).resolve() and cfg['controller_sha256']==sha(controller),'Goal/controller mismatch')
 require(v['configuration_sha256']==sha(root/'supervisor.json'),'Configuration changed')
 denied=str(root.parent/'code-factory-recovery-owner/scripts/queue-fixture-policy.py');history=regular(directory/'native-context-final.json');require(v['denial_history_sha256']==sha(history),'Denial history changed')
 rejected=[]
 for message in read(history):
  for content in message.get('content',[]):
   state=content.get('state',{});error=state.get('error',{})
   if isinstance(error,dict) and error.get('type')=='permission.rejected':rejected.append(content)
 require(len(rejected)==1,'Additional or absent denial');call=rejected[0]
 if v['kind']=='contract-only-checker-reference':
  require(call.get('name')=='read' and call.get('executed') is False and call['state'].get('input')=={'path':denied} and call['state']['error'].get('message')=='Permission denied: external_directory','Different denied action')
 else:
  if host_git:
   require(re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}',out['attempt']) is not None,'Invalid cleanup attempt identity')
   require(call.get('type')=='tool','Different denied action')
   worktree=directory/'controller/workers'/out['attempt']
   require(out['worktree']==str(worktree),'Foreign worktree cleanup target')
   command='cd '+str(root/'repository')+' && git worktree remove --force '+str(worktree)+' 2>&1 | tail -2; echo rc=$?; git worktree prune 2>&1 | tail -1; echo rc2=$?'
   history_rows=read(history)
   require(history_rows[-1].get('content')==[call] and call['state'].get('status')=='error','Actions after denied cleanup or incomplete denial')
   require(v.get('denied_action_sha256')==digest(call),'Denied action fingerprint changed')
   require(v.get('worker_task_sha256')==sha(regular(root/'task.txt')),'Worker Git ownership contract changed')
  else:command='cd '+str(root/'repository')+' && python3 scripts/test-queue-cli.py 2>&1 | tail -15'
  require(call.get('name')=='shell' and call.get('executed') is False and call['state'].get('input')=={'command':command} and call['state']['error'].get('message')=='Permission denied: external_directory','Different denied action')
  require(isinstance(out.get('checkpoint'),str) and len(out['checkpoint'])==40 and v.get('checkpoint')==out['checkpoint'],'Unaccepted checkpoint mismatch')
 manifest_path=regular(v['manifest']);manifest=read(manifest_path);state=read(regular(manifest['state']));require(state['manifest_digest']==digest(manifest),'Owner manifest not reconciled');ownership=[owner_lock(manifest['pool_lock']),owner_lock(root/'supervisor.lock')]
 current=state.get('current')
 require(not current or current.get('goal')==out['goal_id'] and current.get('expected_sequence')==next_sequence,'Another active goal or sequence')
 require(any(g['id']==out['goal_id'] and pathlib.Path(g['root']).resolve()==root for g in manifest['goals']),'Manifest goal mismatch')
 pins=manifest['trusted_files'];require(pins.get(str(proof))==sha(proof),'Resolution is not owner-pinned')
 without=json.loads(json.dumps(manifest));without['trusted_files'].pop(str(proof));require(v['manifest_without_resolution_sha256']==digest(without),'Owner manifest changed')
 def pinned(p):
  p=regular(p);require(pins.get(str(p))==sha(p),'Unpinned resolution input: '+str(p));return read(p)
 require(pins.get(str(pathlib.Path(__file__).resolve()))==sha(__file__),'Unpinned resolver implementation')
 authorization=pinned(v['authorization']);require(authorization.get('decision')=='approve' and authorization.get('source')=='verified-human-conversation' and all(isinstance(authorization.get(k),str) and authorization[k].strip() for k in ['thread_id','message_id','reply']),'Explicit human authorization missing')
 expected={'goal_id':out['goal_id'],'original_sha256':sha(original),'denial_history_sha256':sha(history),'packaging_sha256':v['packaging_sha256'],'next_sequence':next_sequence,'denied_file_remains_inaccessible':True,'missing_usage_remains_unknown':True,'same_goal_only':True}
 if v['kind'] in ['owner-checkout-isolation','host-git-ownership']:
  expected.update(kind=v['kind'],checkpoint=out['checkpoint'],isolation_sha256=v['isolation_sha256'],post_denial_history_preserved=True,denied_command_will_not_be_retried=True)
  if host_git:expected.update(denied_action_sha256=v['denied_action_sha256'],worker_task_sha256=v['worker_task_sha256'],host_owns_git_mutations=True)
 require(authorization.get('scope')==expected,'Authorization scope mismatch')
 packaging=pinned(v['packaging']);require(sha(v['packaging'])==v['packaging_sha256'] and packaging['applied'] is True and packaging['host_checker_bytes_unchanged'] is True and packaging['host_acceptance_wrapper_only_changes_checker_location'] is True and packaging['manifest_goals_acceptance_models_publication_and_permissions_unchanged'] is True and packaging['denied_helper_exposed_to_worker'] is False,'Packaging proof mismatch')
 data=root/'trusted/queue-publication-fixture';host=root/'trusted/queue-publication-host/check.py';contract=data/'VERIFIER-CONTRACT.md'
 require(pathlib.Path(cfg['data']).resolve()==data and sorted(p.name for p in data.iterdir())==['VERIFIER-CONTRACT.md'] and not data.is_symlink(),'Worker inputs are not contract-only')
 for p,h in [(host,packaging['host_checker_sha256']),(contract,packaging['contract_sha256'])]:require(sha(regular(p))==h and pins.get(str(p))==h,'Frozen checker/contract changed')
 for p in [root/'task.txt',root/'trusted/verify.py',root/'supervisor.json']:require(pins.get(str(p))==sha(regular(p)),'Unpinned runtime input')
 require('Read data/check.py' not in (root/'task.txt').read_text() and 'data/VERIFIER-CONTRACT.md' in (root/'task.txt').read_text(),'Stale worker instruction')
 before=pathlib.Path(v['packaging']).parent/'before';prior=pinned(before/'manifest.json')
 require(manifest['goals']==prior['goals'] and manifest.get('main_publication')==prior.get('main_publication'),'Acceptance or publication contract changed')
 require(pins.get(str(before/'check.py'))==sha(regular(before/'check.py'))==sha(host),'Original frozen oracle changed')
 regular(before/'verify.py');require(pins.get(str(before/'verify.py'))==sha(before/'verify.py'),'Unpinned original verifier')
 require((root/'trusted/verify.py').read_text().replace("T/'queue-publication-host/check.py'","T/'queue-publication-fixture/check.py'")== (before/'verify.py').read_text(),'Host verifier changes more than checker location')
 checks=pinned(v['verification']);required=['worker_inputs_contract_only','contract_readable','host_checker_hidden','denied_helper_hidden','contract_readonly','project_writable','git_control_readonly']
 require(checks['passed'] is True and all(checks['checks']['kernel'].get(k) is True for k in required) and checks['checks']['same_goal_admission_gate']['blocked'] is True,'Packaging verification missing')
 if v['kind'] in ['owner-checkout-isolation','host-git-ownership']:
  isolation=pinned(v['isolation']);require(sha(v['isolation'])==v['isolation_sha256'],'Isolation evidence changed')
  kernel=pinned(isolation['kernel_output']);required=['assigned_cwd','worker_writable','owner_source_hidden','owner_canary_hidden','owner_absolute_execution_blocked','owner_symlink_hidden','.git_readonly','frozen.txt_readonly','git_reads_work','git_status_work','git_metadata_readonly']
  require(kernel.get('passed') is True and kernel.get('baseline') is False and all(kernel.get('checks',{}).get(k) is True for k in required),'Owner-source isolation not independently verified')
  execution=pinned(isolation['kernel_receipt']);plan=pinned(isolation['kernel_plan'])
  require(execution.get('observer')=='linux_subreaper_waitpid_pidfd' and execution.get('execution_id')==plan.get('execution_id') and execution.get('root_exit_status')==0 and execution.get('quiescent') is True and execution.get('leaked_descendants') is False and execution.get('cleanup_error') is None and plan.get('stdout')==isolation['kernel_output'],'Kernel verifier execution not confirmed')
  qualified=pinned(isolation['qualification']);require(qualified.get('passed') is True and qualified.get('source_commit')==isolation.get('source_commit') and all(qualified.get('checks',{}).get(k) is True for k in ['candidate','integration','whole_goal','owner_source_hidden','denial_sticky','terminal_denial_blocked']),'Incomplete isolation qualification')
  if host_git:require(all(qualified.get('checks',{}).get(k) is True for k in ['worker_git_contract','host_git_denial_exact','pending_authorization_blocks']),'Host Git ownership recovery not independently qualified')
  published=pinned(isolation['publication']);binding=published.get('binding',{});intent=published.get('intent',{})
  require(binding.get('repository')=='lhemerly/code-factory-controller' and binding.get('repository_id')==1414818646 and binding.get('private') is True and binding.get('branch')=='main' and binding.get('required_checks')==['rust-and-contract'] and intent.get('state')=='confirmed' and intent.get('commit')==isolation['source_commit'] and intent.get('ci',{}).get('status')=='passed','Isolation source not confirmed on private main')
  files=isolation.get('runtime_files',{});expected_files=[pathlib.Path(__file__).resolve().parent/n for n in ['spark-sandbox.mjs','continuous-ownership.mjs']+(['research-context.mjs'] if host_git else [])]
  require(set(files)==set(map(str,expected_files)),'Incomplete isolation runtime inventory')
  for p in expected_files:require(files[str(p)]==sha(regular(p))==pins.get(str(p))==qualified.get('runtime_sha256',{}).get(p.name),'Isolation runtime changed')
 execution=pinned(v['owned_receipt']);plan=pinned(v['owned_plan']);require(execution['execution_id']==plan['execution_id'] and str(manifest_path) in plan['argv'] and execution['quiescent'] is True and execution['leaked_descendants'] is False and type(execution['root_exit_status']) is int,'Owned execution cleanup missing')
 observed=json.loads(subprocess.check_output([str(controller),'inspect','--config',str(directory/'controller.json'),'--job',out['job_id']],text=True,timeout=40));live=observed.get('execution_recovery') or {};require(observed.get('state')=='blocked' and live.get('stop_id')==v['stop_id'] and live.get('attempt_id')==out['attempt'] and live.get('session_id')==out['child'] and live.get('execution_stop',{}).get('status')=='confirmed','Authoritative stop changed')
 require(live.get('accounting',{}).get('status') in ['unknown','settled_zero'],'Unknown verdict or accounting violation')
 # The native context-preflight record for this exact next sequence may already
 # exist, but no admitted or later execution can reuse this resolution.
 for p in (root/'cycles').iterdir():
  if p.name.isdigit() and int(p.name)>=next_sequence:
   require(int(p.name)==next_sequence and not any((p/n).exists() for n in ['controller.json','operator-admit.json','operator-prepare.json','native-session.json']),'Resolution already consumed or superseded')
 require(not (root/'PAUSE').exists(),'Explicit user pause remains authoritative')
 return {'kind':v['kind'],'owner_locks':ownership,'proof_sha256':sha(proof),'authorization_sha256':sha(v['authorization']),'original_sha256':sha(original),'denial_history_sha256':sha(history),'packaging_sha256':v['packaging_sha256'],'goal_id':out['goal_id'],'authorized_next_sequence':next_sequence,'prior_outcome':'failed','prior_denial_preserved':True,'accounting':live['accounting']['status'],'usage':out.get('usage'),'access_grants':[],'acceptance_granted':False}

def main():
 p=argparse.ArgumentParser();p.add_argument('--root',required=True);p.add_argument('--directory',required=True);p.add_argument('--controller',required=True);p.add_argument('--next-sequence',required=True,type=int);a=p.parse_args();print(json.dumps(audit(a.root,a.directory,a.controller,a.next_sequence)))
if __name__=='__main__':main()
