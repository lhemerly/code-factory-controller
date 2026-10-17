import shutil,subprocess,fcntl,copy,hashlib,importlib.util,json,pathlib,tempfile,unittest
P=pathlib.Path(__file__).resolve().parent/'permission-block-resolution.py';s=importlib.util.spec_from_file_location('resolution',P);M=importlib.util.module_from_spec(s);s.loader.exec_module(M)
class PermissionResolutionRegression(unittest.TestCase):
 def fixture(self):
  temp=tempfile.TemporaryDirectory();self.addCleanup(temp.cleanup);base=pathlib.Path(temp.name);root=base/'goal';cycle=root/'cycles/000003';cycle.mkdir(parents=True);data=root/'trusted/queue-publication-fixture';data.mkdir(parents=True);host=root/'trusted/queue-publication-host';host.mkdir();put=lambda p,x:p.write_text(json.dumps(x))
  controller=base/'controller';status=base/'live.json';controller.write_text('#!/usr/bin/python3\nimport pathlib\nprint(pathlib.Path('+repr(str(status))+').read_text())\n');controller.chmod(0o700)
  cfg={'controller':str(controller),'controller_sha256':M.sha(controller),'data':str(data)};put(root/'supervisor.json',cfg);put(root/'goal.json',{'id':'synthetic-goal'});put(cycle/'controller.json',{})
  out={'status':'failed','failure_kind':'blocked','phase':'native','cleanup_confirmed':True,'sequence':3,'goal_id':'synthetic-goal','job_id':'synthetic-job','attempt':'synthetic-attempt','child':'synthetic-session','parent':'synthetic-parent','worktree':str(base/'worker'),'error':'Permission denied in native tool history','usage':{'accounting_status':'unknown','tokens':{'input':5},'missing':None}}
  put(cycle/'result.json',out);denied=str(base/'code-factory-recovery-owner/scripts/queue-fixture-policy.py');put(cycle/'native-context-final.json',[{'content':[{'name':'read','executed':False,'state':{'input':{'path':denied},'error':{'type':'permission.rejected','message':'Permission denied: external_directory'}}}]}]);(data/'VERIFIER-CONTRACT.md').write_text('Frozen output requirements');(host/'check.py').write_text('# Frozen independent oracle');(root/'trusted/verify.py').write_text('# Host invocation');(root/'task.txt').write_text('Read data/VERIFIER-CONTRACT.md. Host verifies independently.')
  packaging=base/'packaging.json';put(packaging,{'applied':True,'host_checker_bytes_unchanged':True,'host_acceptance_wrapper_only_changes_checker_location':True,'manifest_goals_acceptance_models_publication_and_permissions_unchanged':True,'denied_helper_exposed_to_worker':False,'host_checker_sha256':M.sha(host/'check.py'),'contract_sha256':M.sha(data/'VERIFIER-CONTRACT.md')})
  verification=base/'verification.json';put(verification,{'passed':True,'checks':{'kernel':{'contract_only':True,'helper_hidden':True,'readonly':True},'same_goal_admission_gate':{'blocked':True}}})
  pool=base/'pool.lock';goal_lock=root/'supervisor.lock';locks=[]
  for lock in [pool,goal_lock]:
   fd=lock.open('a');fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB);locks.append(fd);self.addCleanup(fd.close)
  manifest=base/'launch.json';owner_state=base/'dispatcher-state.json';receipt=base/'owned.json';plan=base/'owned-plan.json';put(receipt,{'execution_id':'synthetic-owned','quiescent':True,'leaked_descendants':False,'root_exit_status':0});put(plan,{'execution_id':'synthetic-owned','argv':['dispatcher','run','--manifest',str(manifest)]})
  authorization=base/'authorization.json';scope={'goal_id':out['goal_id'],'original_sha256':M.sha(cycle/'result.json'),'denial_history_sha256':M.sha(cycle/'native-context-final.json'),'packaging_sha256':M.sha(packaging),'next_sequence':4,'denied_file_remains_inaccessible':True,'missing_usage_remains_unknown':True,'same_goal_only':True};put(authorization,{'decision':'approve','source':'verified-human-conversation','thread_id':'synthetic-thread','message_id':'synthetic-user-message','reply':'yes','scope':scope})
  proof=root/M.NAME;v={'version':1,'kind':'contract-only-checker-reference','sequence':3,'original_sha256':scope['original_sha256'],'identity':{k:out[k] for k in ['goal_id','job_id','attempt','child','parent','worktree']},'configuration_sha256':M.sha(root/'supervisor.json'),'denial_history_sha256':scope['denial_history_sha256'],'manifest':str(manifest),'authorization':str(authorization),'packaging':str(packaging),'packaging_sha256':M.sha(packaging),'verification':str(verification),'owned_receipt':str(receipt),'owned_plan':str(plan),'stop_id':'synthetic-stop'}
  m={'controller':str(controller),'pool_lock':str(pool),'goals':[{'id':out['goal_id'],'root':str(root)}],'state':str(owner_state),'trusted_files':{str(p):M.sha(p) for p in [P,authorization,packaging,verification,receipt,plan,root/'task.txt',root/'trusted/verify.py',root/'supervisor.json',host/'check.py',data/'VERIFIER-CONTRACT.md']}}
  def seal():
   current=M.read(manifest) if manifest.exists() else copy.deepcopy(m);current['trusted_files'].pop(str(proof),None)
   # Test owner pinning is explicit; only inputs already pinned may be rehashed.
   for p in current['trusted_files']:current['trusted_files'][p]=M.sha(p)
   value=M.read(proof) if proof.exists() else copy.deepcopy(v);value['manifest_without_resolution_sha256']=M.digest(current);put(proof,value);current['trusted_files'][str(proof)]=M.sha(proof);put(manifest,current);put(owner_state,{'manifest_digest':M.digest(current)})
  seal();live={'state':'blocked','execution_recovery':{'stop_id':'synthetic-stop','attempt_id':out['attempt'],'session_id':out['child'],'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}};put(status,live)
  return locals()
 def audit(self,f,next_sequence=4):return M.audit(f['root'],f['cycle'],f['controller'],next_sequence)
 def test_preserves_original_unknown_usage_and_denial(self):
  f=self.fixture();before=(f['cycle']/'result.json').read_bytes();r=self.audit(f);self.assertEqual(r['accounting'],'unknown');self.assertIsNone(r['usage']['missing']);self.assertEqual(r['access_grants'],[]);self.assertFalse(r['acceptance_granted']);self.assertEqual(before,(f['cycle']/'result.json').read_bytes());self.assertTrue(r['prior_denial_preserved'])
 def test_next_preflight_directory_allowed_but_admission_consumes_resolution(self):
  f=self.fixture();n=f['root']/'cycles/000004';n.mkdir();self.audit(f);(n/'controller.json').write_text('{}')
  with self.assertRaisesRegex(ValueError,'consumed'):self.audit(f)
 def test_stop_and_accounting_controls(self):
  for key,value in [('stop_id','other'),('attempt_id','other'),('session_id','other'),('execution_stop',{'status':'unknown'}),('accounting',{'status':'violating'}),('accounting',{})]:
   with self.subTest(key=key,value=value):
    f=self.fixture();live=M.read(f['status']);live['execution_recovery'][key]=value;f['put'](f['status'],live)
    with self.assertRaises(ValueError):self.audit(f)
 def test_owner_authorization_and_evidence_controls(self):
  cases=[('authorization','decision','pending'),('authorization','source','worker-prose'),('authorization','message_id',''),('proof','kind','force-resume'),('proof','original_sha256','changed'),('proof','denial_history_sha256','changed'),('proof','sequence',2),('proof','configuration_sha256','changed'),('packaging','denied_helper_exposed_to_worker',True),('packaging','host_checker_bytes_unchanged',False),('packaging','manifest_goals_acceptance_models_publication_and_permissions_unchanged',False),('packaging','contract_sha256','changed'),('verification','passed',False),('receipt','quiescent',False),('receipt','leaked_descendants',True),('receipt','execution_id','other')]
  for file,key,value in cases:
   with self.subTest(file=file,key=key):
    f=self.fixture();d=M.read(f[file]);d[key]=value;f['put'](f[file],d);f['seal']()
    with self.assertRaises(ValueError):self.audit(f)
 def test_no_unpinned_or_symlink_resolution_and_no_user_pause(self):
  for kind in ['pin','symlink','pause','extra-data','checker-changed','original-changed','additional-denial','next','later','authorization-scope','pool-unlocked','goal-unlocked','other-owner-sequence']:
   with self.subTest(kind=kind):
    f=self.fixture()
    if kind=='pin':m=M.read(f['manifest']);m['trusted_files'].pop(str(f['proof']));f['put'](f['manifest'],m);f['put'](f['owner_state'],{'manifest_digest':M.digest(m)})
    elif kind=='symlink':f['proof'].rename(f['base']/'saved');f['proof'].symlink_to(f['base']/'saved')
    elif kind=='pause':f['put'](f['root']/'PAUSE',{'reason':'User paused'})
    elif kind=='extra-data':(f['data']/'check.py').write_text('unexpected exposed checker')
    elif kind=='checker-changed':(f['host']/'check.py').write_text('weakened')
    elif kind=='original-changed':(f['cycle']/'result.json').write_text('{}')
    elif kind=='additional-denial':d=M.read(f['cycle']/'native-context-final.json');d.append(d[0]);f['put'](f['cycle']/'native-context-final.json',d)
    elif kind=='next':
     with self.assertRaises(ValueError):self.audit(f,5)
     continue
    elif kind=='later':(f['root']/'cycles/000005').mkdir()
    elif kind=='pool-unlocked':fcntl.flock(f['locks'][0],fcntl.LOCK_UN)
    elif kind=='goal-unlocked':fcntl.flock(f['locks'][1],fcntl.LOCK_UN)
    elif kind=='other-owner-sequence':d=M.read(f['owner_state']);d['current']={'goal':'other','expected_sequence':4};f['put'](f['owner_state'],d)
    elif kind=='authorization-scope':d=M.read(f['authorization']);d['scope']['same_goal_only']=False;f['put'](f['authorization'],d);f['seal']()
    with self.assertRaises((ValueError,KeyError)):self.audit(f)
 def test_context_admission_uses_audited_resolution_and_keeps_failure(self):
  f=self.fixture();repo=f['root']/'repository';repo.mkdir()
  def git(*args):return subprocess.check_output(['/usr/bin/git','-C',str(repo),'-c','user.name=Synthetic','-c','user.email=synthetic@localhost',*args],text=True).strip()
  git('init','-q');(repo/'product.py').write_text('synthetic=1');git('add','.');git('commit','-qm','synthetic');base=git('rev-parse','HEAD')
  cfg=M.read(f['root']/'supervisor.json');cfg['initial_base']=base;f['put'](f['root']/'supervisor.json',cfg)
  goal={'id':'synthetic-goal','objective':'Useful synthetic work','completion_criteria':['independent acceptance'],'pause_conditions':['missing input']};f['put'](f['root']/'goal.json',goal)
  out=M.read(f['cycle']/'result.json');out.update(ended_utc='2026-01-01T00:00:00Z',base=base,recovery=f['live']['execution_recovery']);f['put'](f['cycle']/'result.json',out)
  v=M.read(f['proof']);v['original_sha256']=M.sha(f['cycle']/'result.json');v['configuration_sha256']=M.sha(f['root']/'supervisor.json');f['put'](f['proof'],v)
  auth=M.read(f['authorization']);auth['scope']['original_sha256']=v['original_sha256'];f['put'](f['authorization'],auth);f['seal']()
  data=f['base']/'context-input.json';f['put'](data,{'root':str(f['root']),'repo':str(repo),'state':{'base':base},'goal':goal,'cfg':cfg,'beforeSequence':4})
  code="import {readFileSync} from 'node:fs';import {buildResearchContext} from "+json.dumps((P.parent/'research-context.mjs').as_uri())+";const c=await buildResearchContext(JSON.parse(readFileSync(process.argv[1])));console.log(JSON.stringify(c));"
  context=json.loads(subprocess.check_output([shutil.which('node'),'--input-type=module','-e',code,str(data)],text=True));resolution=context['provenance']['sources'][0]['permission_resolution'];self.assertEqual(resolution['accounting'],'unknown');self.assertTrue(resolution['prior_denial_preserved']);self.assertIn('unknown',context['text']);self.assertEqual(M.read(f['cycle']/'result.json')['failure_kind'],'blocked')
  auth['decision']='pending';f['put'](f['authorization'],auth);f['seal']();p=subprocess.run([shutil.which('node'),'--input-type=module','-e',code,str(data)],capture_output=True,text=True);self.assertNotEqual(p.returncode,0);self.assertIn('owner-pinned resolution audit failed',p.stderr)
 def test_owner_resume_rejects_before_mutating_pause(self):
  from unittest import mock
  spec=importlib.util.spec_from_file_location('resolution_dispatch',P.parent/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D)
  f=self.fixture();out=M.read(f['cycle']/'result.json');out['recovery']=f['live']['execution_recovery'];f['put'](f['cycle']/'result.json',out)
  v=M.read(f['proof']);v['original_sha256']=M.sha(f['cycle']/'result.json');f['put'](f['proof'],v);a=M.read(f['authorization']);a['scope']['original_sha256']=v['original_sha256'];a['decision']='pending';f['put'](f['authorization'],a);f['seal']()
  native={'status':'paused','sequence':3,'current':None,'last_cycle':{'result':str(f['cycle']/'result.json')}};f['put'](f['root']/'status.json',native);f['locks'][1].close()
  with mock.patch.object(D.subprocess,'run') as run:
   with self.assertRaisesRegex(ValueError,'authorization'):D.owner_resume(M.read(f['manifest']))
   run.assert_not_called()
  self.assertEqual(M.read(f['root']/'status.json'),native)
 def test_exact_denial_cannot_resolve_another_action(self):
  for field,value in [('name','edit'),('executed',True),('path','/synthetic-other/file'),('error','Permission denied: shell')]:
   with self.subTest(field=field):
    f=self.fixture();history=M.read(f['cycle']/'native-context-final.json');call=history[0]['content'][0]
    if field=='path':call['state']['input']['path']=value
    elif field=='error':call['state']['error']['message']=value
    else:call[field]=value
    f['put'](f['cycle']/'native-context-final.json',history);h=M.sha(f['cycle']/'native-context-final.json');v=M.read(f['proof']);v['denial_history_sha256']=h;f['put'](f['proof'],v);a=M.read(f['authorization']);a['scope']['denial_history_sha256']=h;f['put'](f['authorization'],a);f['seal']()
    with self.assertRaisesRegex(ValueError,'Different denied action'):self.audit(f)
 def test_authorization_cannot_waive_unknown_usage_or_expose_file(self):
  for field,value in [('missing_usage_remains_unknown',False),('denied_file_remains_inaccessible',False),('next_sequence',5),('goal_id','other')]:
   with self.subTest(field=field):
    f=self.fixture();a=M.read(f['authorization']);a['scope'][field]=value;f['put'](f['authorization'],a);f['seal']()
    with self.assertRaisesRegex(ValueError,'Authorization scope'):self.audit(f)
if __name__=='__main__':unittest.main(verbosity=2)
