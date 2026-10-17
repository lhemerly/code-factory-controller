"""Owner-only reconciliation of a reserved identifier that never became a job.

This records absence, not native-stop or billing evidence. Controller ledger,
owned-process receipts and existing locks must all agree before reassignment.
"""
import argparse,contextlib,fcntl,hashlib,importlib.util,json,os,pathlib,sqlite3,stat,subprocess
S=pathlib.Path(__file__).resolve().parent
def read(p):return json.loads(pathlib.Path(p).read_text())
def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def module(name,file):
 sp=importlib.util.spec_from_file_location(name,S/file);m=importlib.util.module_from_spec(sp);sp.loader.exec_module(m);return m
save=module('admission_storage','goal-runner.py').save
EMPTY=('jobs','attempts','grants','requests','receipts','events','steps')
FIELDS=('attempt','session','child','parent','worktree','candidate','checkpoint','native_session','acceptance')

def context_rejection(root,directory):
 root=pathlib.Path(root).resolve();directory=pathlib.Path(directory).resolve();out=read(directory/'result.json')
 if (directory.parent!=root/'cycles' or out.get('goal_id')!=read(root/'goal.json')['id'] or out.get('phase')!='context_preflight'
     or out.get('status')!='failed' or out.get('cleanup_confirmed') is not True or out.get('admission_requested') is not False
     or not out.get('ended_utc') or out.get('job_id') is not None or out.get('fence_error') or any(out.get(k) is not None for k in FIELDS)):
  raise ValueError('Not a completed context rejection before admission')
 if any((directory/name).exists() for name in ['controller.json','host-private.json','operator-admit.json','operator-prepare.json','native-session.json']):raise ValueError('Controller/session evidence exists')
 return {'kind':'not_admitted','sequence':out['sequence'],'goal_id':out['goal_id'],'worker_session_created':False,'original_result_sha256':sha(directory/'result.json')}

def absence(root,directory,controller):
 root=pathlib.Path(root).resolve();directory=pathlib.Path(directory).resolve()
 if directory.parent!=root/'cycles':raise ValueError('Exact cycle directory required')
 result=directory/'result.json';out=read(result);goal=read(root/'goal.json');cfg=read(directory/'controller.json');supervisor=read(root/'supervisor.json')
 if (out.get('goal_id')!=goal['id'] or str(out.get('sequence')).zfill(6)!=directory.name or out.get('status')!='failed'
     or out.get('phase')!='admission' or out.get('cleanup_confirmed') is not True or any(out.get(k) is not None for k in FIELDS)):
  raise ValueError('Not an owned failure before worker preparation')
 # Only the observed setup failure is eligible, never a permission/ownership denial.
 if out.get('error') not in ['Error: Project instructions exceed admission budget; no instructions were truncated','Error: Required goal and outcome context exceeds admission budget; no context was truncated']:
  raise ValueError('Failure is outside the approved pre-admission recovery class')
 if out.get('fence_error') not in [None,'Error: Controller cancel: {"error":"job not found"}']:raise ValueError('Unrecognized fence remains unresolved')
 if not out.get('job_id') or not out.get('ended_utc'):raise ValueError('Reserved identifier/terminal record missing')
 if (cfg.get('project_id')!=goal['id'] or pathlib.Path(cfg['data_dir']).resolve()!=directory/'controller'
     or pathlib.Path(cfg['repository']).resolve()!=root/'repository' or pathlib.Path(supervisor['controller']).resolve()!=pathlib.Path(controller).resolve()
     or sha(controller)!=supervisor['controller_sha256']):raise ValueError('Controller/goal identity changed')
 for marker in ['operator-admit.json','operator-prepare.json','operator-bind-v2.json','host-private.json','native-session.json']:
  if (directory/marker).exists():raise ValueError('Admission/session evidence exists; use native execution recovery')
 lock=directory/'controller/controller.lock';db=directory/'controller/controller.sqlite'
 if lock.is_symlink() or db.is_symlink() or not lock.is_file() or not db.is_file():raise ValueError('Original controller ledger/lock missing or redirected')
 # The existing controller validates its own persisted configuration fingerprint.
 # No missing-file error or arbitrary command failure counts as proof of absence.
 with lock.open('r') as held:fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
 inspected=subprocess.run([str(controller),'inspect','--config',str(directory/'controller.json'),'--job',out['job_id']],capture_output=True,text=True,timeout=30)
 if inspected.returncode!=1 or inspected.stdout.strip() or inspected.stderr.strip()!='Error: job not found\n\nCaused by:\n    Query returned no rows':raise ValueError('Controller did not authoritatively confirm absent job')
 with lock.open('r') as held:
  fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
  with contextlib.closing(sqlite3.connect(db.as_uri()+'?mode=ro',uri=True)) as conn:
   conn.execute('BEGIN');counts={table:conn.execute('SELECT count(*) FROM '+table).fetchone()[0] for table in EMPTY}
   if any(counts.values()):raise ValueError('Controller records a job, attempt, grant, request or execution effect')
 return {'kind':'not_admitted','goal_id':goal['id'],'sequence':out['sequence'],'job_id':out['job_id'],
         'original_result_sha256':sha(result),'controller_config_sha256':sha(directory/'controller.json'),
         'controller_sha256':sha(controller),'ledger_counts':counts,'worker_session_created':False}

def owner_receipt(path,manifest_path):
 path=pathlib.Path(path).resolve();plan_path=path.with_name(path.name.replace('.process.json','.plan.json'))
 receipt=read(path);plan=read(plan_path)
 if (receipt.get('observer')!='linux_subreaper_waitpid_pidfd' or receipt.get('quiescent') is not True or type(receipt.get('root_exit_status')) is not int
     or receipt.get('leaked_descendants') is not False or receipt.get('cleanup_error') is not None
     or receipt.get('execution_id')!=plan.get('execution_id') or pathlib.Path(plan['receipt']).resolve()!=path):
  raise ValueError('Positive owned-process quiescence required')
 argv=plan['argv']
 if '--manifest' not in argv or pathlib.Path(argv[argv.index('--manifest')+1]).resolve()!=pathlib.Path(manifest_path).resolve():raise ValueError('Owned execution belongs to another manifest')
 return {'path':str(path),'sha256':sha(path),'plan':str(plan_path),'plan_sha256':sha(plan_path),'execution_id':receipt['execution_id']}

def audit(root,directory,controller):
 directory=pathlib.Path(directory)
 if read(directory/'result.json').get('phase')=='context_preflight':return context_rejection(root,directory)
 att=read(directory/'pre-admission.json')
 fresh=absence(root,directory,controller)
 if att.get('absence')!=fresh:raise ValueError('Pre-admission evidence changed')
 owned=att['owned'];observed=owner_receipt(owned['path'],att['manifest'])
 if observed!=owned:raise ValueError('Pre-admission owned receipt changed')
 return fresh

def reconcile(manifest_path,goal_id,owned_path):
 D=module('pre_admission_dispatch','research-dispatch.py');manifest_path=pathlib.Path(manifest_path).resolve();m=read(manifest_path)
 for p,h in m['trusted_files'].items():
  if sha(p)!=h:raise ValueError('Trusted runtime/configuration pin changed')
 goal=next(g for g in m['goals'] if g['id']==goal_id);root=pathlib.Path(goal['root']).resolve();state_path=pathlib.Path(m['state']);owned=owner_receipt(owned_path,manifest_path)
 locks=D.Locks()
 try:
  for p in [m['pool_lock'],str(root/'supervisor.lock')]:locks.acquire(p)
  journal=root/'pre-admission-journal.json'
  if journal.exists():
   old=read(journal);save(root/'status.json',old['status']);save(state_path,old['state']);journal.unlink();raise ValueError('Interrupted reconciliation restored; audit before retry')
  status=read(root/'status.json');state=read(state_path);current=state.get('current') or {}
  if (root/'PAUSE').exists() or current.get('goal')!=goal_id or current.get('expected_sequence')!=status['sequence']:raise ValueError('Exact stopped cycle required; user pause remains authoritative')
  directory=root/'cycles'/str(status['sequence']).zfill(6);proof=absence(root,directory,m['controller']);out=read(directory/'result.json')
  expected='Execution/recovery evidence incomplete: '+out['error']
  if status.get('status')!='paused' or status.get('pause_reason')!=expected:raise ValueError('Unrelated/native permission pause cannot be cleared')
  # Journal both mutable owner records; original result and stop/accounting fields
  # are never changed. A later invocation restores a partial write before retry.
  att={'version':1,'absence':proof,'owned':owned,'manifest':str(manifest_path)};save(directory/'pre-admission.json',att)
  before={'status':status,'state':state};save(journal,before)
  status={**status,'status':'waiting','current':None,'last_cycle':{'sequence':out['sequence'],'status':'failed','result':str(directory/'result.json')}};status.pop('pause_reason',None)
  status.setdefault('pre_admission_reconciliations',[]).append({'sequence':out['sequence'],'result_sha256':proof['original_result_sha256'],'kind':'not_admitted'})
  state={**state,'current':None,'status':'waiting'};state.pop('reason',None);gs=state['goals'][goal_id];gs.update(state='ready',last_result=str(directory/'result.json'),native_status='not_admitted',usage=out.get('usage'),model=out.get('model'));gs.pop('reason',None)
  save(root/'status.json',status);save(state_path,state);journal.unlink()
  return proof
 finally:locks.close()

def advance_scope(manifest_path,goal_id,published_path,new_base):
 """Merge an already verified published baseline without widening writable paths.

 Required after publishing the recovery implementation while a queued repair
 input still descends from its older baseline. No acceptance is granted here.
 """
 D=module('scope_dispatch','research-dispatch.py');mp=pathlib.Path(manifest_path).resolve();m=read(mp);goal=next(g for g in m['goals'] if g['id']==goal_id);root=pathlib.Path(goal['root']).resolve();repo=root/'repository';published=read(published_path);base=published.get('intent',{}).get('commit');binding=published.get('binding',{})
 if (binding.get('repository')!='lhemerly/code-factory-controller' or binding.get('repository_id')!=1414818646 or binding.get('private') is not True
     or binding.get('branch')!='main' or binding.get('required_checks')!=['rust-and-contract'] or published.get('intent',{}).get('state')!='confirmed' or published['intent'].get('ci',{}).get('status')!='passed'):
  raise ValueError('Exact verified private-main publication required')
 for oid in [base,new_base]:
  if not isinstance(oid,str) or len(oid)!=40 or any(c not in '0123456789abcdef' for c in oid):raise ValueError('Exact source commits required')
 locks=D.Locks()
 try:
  for p in [m['pool_lock'],str(root/'supervisor.lock')]:locks.acquire(p)
  journal=root/'scope-base-journal.json'
  if journal.exists():
   old=read(journal)
   if pathlib.Path(old['manifest']['state']).resolve()!=pathlib.Path(m['state']).resolve():raise ValueError('Baseline journal belongs to another owner')
   for name in ['verified-scope-publication.json','approved-policy-migration.json']:
    value=old['files'][name];target=root/name
    if value is None:D.queue_api.unlink_durable(target)
    else:D.queue_api.write_bytes(target,value.encode())
   save(root/'supervisor.json',old['configuration']);save(root/'status.json',old['status']);save(pathlib.Path(m['state']),old['state']);save(mp,old['manifest']);D.queue_api.unlink_durable(journal)
   raise ValueError('Interrupted baseline transaction restored; audit before retry')
  for p,h in m['trusted_files'].items():
   if sha(p)!=h:raise ValueError('Existing trusted pin changed')
  status=read(root/'status.json');state=read(m['state']);cfg=read(root/'supervisor.json');goal_document=read(root/'goal.json')
  if status.get('current') or state.get('current') or (root/'PAUSE').exists():raise ValueError('Quiescent reconciled owner required')
  def git(*args):return subprocess.check_output(['/usr/bin/git','-C',str(repo),*args],text=True).strip()
  git('merge-base','--is-ancestor',base,new_base);git('merge-base','--is-ancestor',status['base'],new_base)
  template=read(root/'job-template.json');paths=git('diff','--name-only',base,new_base).splitlines()
  if any(not any(p==a or p.startswith(a+'/') for a in template['allowed_paths']) or any(p==a or p.startswith(a+'/') for a in template['protected_paths']) for p in paths):raise ValueError('Verified source advance would widen repair scope')
  fingerprint=lambda c:hashlib.sha256(json.dumps({'cfg':c,'goal':goal_document},ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
  if status['policy_hash']!=fingerprint(cfg):raise ValueError('Unreconciled native policy change')
  after={**cfg,'scope_base':base};snapshot=root/'verified-scope-publication.json';migration=root/'approved-policy-migration.json'
  approval={'kind':'verified-scope-base-advance','from':status['policy_hash'],'to':fingerprint(after),'prior_configuration':cfg,'publication_sha256':sha(published_path)}
  save(journal,{'manifest':m,'state':state,'status':status,'configuration':cfg,'files':{name:(root/name).read_text() if (root/name).exists() else None for name in ['verified-scope-publication.json','approved-policy-migration.json']}})
  save(snapshot,published);save(migration,approval);save(root/'supervisor.json',after)
  # save() formatting is canonical; bind the exact snapshot bytes used by native.
  approval['publication_sha256']=sha(snapshot);save(migration,approval)
  updated=json.loads(json.dumps(m))
  for p in [snapshot,migration,root/'supervisor.json']:updated['trusted_files'][str(p)]=sha(p)
  state['manifest_digest']=D.digest(updated);state.setdefault('implementation_migrations',[]).append({'kind':'verified-scope-base-advance','from':D.digest(m),'to':D.digest(updated),'published':base,'repair_input':new_base})
  status['base']=new_base;save(root/'status.json',status);save(pathlib.Path(m['state']),state);save(mp,updated);journal.unlink()
  return {'scope_base':base,'unaccepted_repair_input':new_base,'allowed_paths_unchanged':True,'goal_and_publication_binding_unchanged':True}
 finally:locks.close()

def main():
 p=argparse.ArgumentParser();sub=p.add_subparsers(dest='action',required=True)
 a=sub.add_parser('audit');a.add_argument('--root',required=True);a.add_argument('--directory',required=True);a.add_argument('--controller',required=True)
 a=sub.add_parser('reconcile');a.add_argument('--manifest',required=True);a.add_argument('--goal',required=True);a.add_argument('--owned-receipt',required=True)
 a=sub.add_parser('advance-scope');a.add_argument('--manifest',required=True);a.add_argument('--goal',required=True);a.add_argument('--published-state',required=True);a.add_argument('--repair-input',required=True)
 a=p.parse_args()
 if a.action=='audit':result=audit(a.root,a.directory,a.controller)
 elif a.action=='reconcile':result=reconcile(a.manifest,a.goal,a.owned_receipt)
 else:result=advance_scope(a.manifest,a.goal,a.published_state,a.repair_input)
 print(json.dumps(result))
if __name__=='__main__':main()
