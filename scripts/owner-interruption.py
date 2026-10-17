"""Audit an explicit owner's stopped attempt; never resolve a native access denial.
The failed result and all tool history stay immutable. An owner-pinned record is
required at both resume and admission. No permission, acceptance or usage grant.
"""
import argparse,importlib.util,json,os,pathlib,subprocess
P=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('interruption_evidence',P/'permission-block-resolution.py');E=importlib.util.module_from_spec(spec);spec.loader.exec_module(E)
NAME='owner-interruption.json'

def audit(root,directory,controller,next_sequence):
 root=pathlib.Path(root).resolve();directory=pathlib.Path(directory).resolve()
 proof=E.regular(root/NAME);v=E.read(proof)
 E.require(v.get('version')==1 and v.get('kind')=='owner-interruption','Unsupported interruption record')
 E.require(directory.parent==root/'cycles' and directory.name.isdigit(),'Foreign interruption cycle')
 original=E.regular(directory/'result.json');out=E.read(original);sequence=int(directory.name)
 E.require(out.get('sequence')==v.get('sequence')==sequence and next_sequence==sequence+1 and E.sha(original)==v.get('original_sha256'),'Stale interruption identity')
 E.require(out.get('status')=='failed' and out.get('failure_kind')=='blocked' and out.get('phase')=='native' and out.get('error')=='Error: Operator stopping','Not an owner-interrupted native attempt')
 E.require(out.get('cleanup_confirmed') is True and not out.get('fence_error') and not out.get('candidate') and not out.get('acceptance'),'Unconfirmed or accepted interruption')
 identity={k:out.get(k) for k in ['goal_id','job_id','attempt','child','parent','worktree']}
 E.require(identity==v.get('identity') and all(isinstance(x,str) and x for x in identity.values()),'Interrupted execution identity mismatch')
 history=E.regular(directory/'native-context-final.json');E.require(E.sha(history)==v.get('history_sha256'),'Interrupted history changed')
 context=E.read(history);E.require(isinstance(context,list) and context,'Missing final native history')
 # Deliberately conservative: no tool error, including unknown kinds, can be
 # converted into an owner interruption. Ordinary nonzero shell exits remain
 # completed native tool calls, not access-denial exceptions.
 calls=[c for m in context for c in m.get('content',[]) if c.get('type')=='tool']
 E.require(calls and all(c.get('state',{}).get('status')=='completed' and not c.get('state',{}).get('error') for c in calls),'Native denial, incomplete tool or unknown tool failure remains fenced')
 manifest_path=E.regular(v['manifest']);manifest=E.read(manifest_path);state=E.read(E.regular(manifest['state']))
 E.require(state.get('manifest_digest')==E.digest(manifest),'Owner manifest not reconciled')
 locks=[E.owner_lock(manifest['pool_lock']),E.owner_lock(root/'supervisor.lock')]
 current=state.get('current');E.require(not current or current.get('goal')==out['goal_id'] and current.get('expected_sequence')==next_sequence,'Another active goal or sequence')
 E.require(any(g['id']==out['goal_id'] and pathlib.Path(g['root']).resolve()==root for g in manifest['goals']),'Manifest goal mismatch')
 pins=manifest['trusted_files'];E.require(pins.get(str(proof))==E.sha(proof),'Interruption is not owner-pinned')
 without=json.loads(json.dumps(manifest));without['trusted_files'].pop(str(proof));E.require(E.digest(without)==v.get('manifest_without_interruption_sha256'),'Owner manifest changed')
 def pinned(p):
  p=E.regular(p);E.require(pins.get(str(p))==E.sha(p),'Unpinned interruption input: '+str(p));return E.read(p)
 E.require(pins.get(str(pathlib.Path(__file__).resolve()))==E.sha(__file__),'Unpinned interruption implementation')
 cfg=pinned(root/'supervisor.json');E.require(pathlib.Path(cfg['controller']).resolve()==pathlib.Path(controller).resolve() and cfg['controller_sha256']==E.sha(controller),'Controller identity changed')
 owner=pinned(v['owner_stop']);execution=pinned(v['owned_receipt']);plan=pinned(v['owned_plan'])
 E.require(owner.get('action')=='SIGTERM exact owned foreground dispatcher' and owner.get('new_permission_denial') is False,'Stop was not an explicit owner interruption')
 E.require(execution.get('observer')=='linux_subreaper_waitpid_pidfd' and execution.get('execution_id')==plan.get('execution_id') and execution.get('quiescent') is True and execution.get('leaked_descendants') is False and execution.get('cleanup_error') is None and type(execution.get('root_exit_status')) is int,'Owned process cleanup unconfirmed')
 stopped=owner.get('owner',{});supervisor=execution.get('supervisor',{})
 E.require(stopped.get('pid')==execution.get('root_pid') and type(stopped.get('start_ticks')) is int and type(supervisor.get('start_ticks')) is int and stopped['start_ticks']>=supervisor['start_ticks'] and stopped.get('uid')==os.getuid() and stopped.get('boot_id')==supervisor.get('boot_id') and str(manifest_path) in plan.get('argv',[]),'Stopped owner/receipt identity mismatch')
 E.require(isinstance(v.get('pause_sha256'),str) and len(v['pause_sha256'])==64,'Missing exact owner pause identity')
 pause=root/'PAUSE'
 if pause.exists():E.require(E.sha(E.regular(pause))==v['pause_sha256'],'Different explicit pause remains authoritative')
 observed=json.loads(subprocess.check_output([str(controller),'inspect','--config',str(directory/'controller.json'),'--job',out['job_id']],text=True,timeout=40));live=observed.get('execution_recovery') or {}
 E.require(observed.get('state')=='blocked' and live.get('stop_id')==v.get('stop_id')==out.get('recovery',{}).get('stop_id') and live.get('attempt_id')==out['attempt'] and live.get('session_id')==out['child'] and live.get('execution_stop',{}).get('status')=='confirmed','Authoritative interruption stop changed')
 E.require(live.get('accounting',{}).get('status') in ['unknown','settled_zero'],'Unknown accounting verdict or known policy violation')
 for p in (root/'cycles').iterdir():
  if p.name.isdigit() and int(p.name)>=next_sequence:E.require(int(p.name)==next_sequence and not any((p/n).exists() for n in ['controller.json','operator-admit.json','operator-prepare.json','native-session.json']),'Interruption record already consumed or superseded')
 return {'kind':'owner-interruption','owner_locks':locks,'proof_sha256':E.sha(proof),'original_sha256':E.sha(original),'history_sha256':E.sha(history),'goal_id':out['goal_id'],'authorized_next_sequence':next_sequence,'prior_outcome':'failed','prior_history_preserved':True,'accounting':live['accounting']['status'],'usage':out.get('usage'),'access_grants':[],'acceptance_granted':False}

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--root',required=True);p.add_argument('--directory',required=True);p.add_argument('--controller',required=True);p.add_argument('--next-sequence',required=True,type=int);a=p.parse_args();print(json.dumps(audit(a.root,a.directory,a.controller,a.next_sequence)))
