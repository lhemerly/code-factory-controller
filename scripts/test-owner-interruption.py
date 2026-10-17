import copy,fcntl,hashlib,importlib.util,json,os,pathlib,shutil,subprocess,tempfile,unittest
P=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('owner_interruption',P/'owner-interruption.py');M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M);E=M.E
class OwnerInterruptionRegression(unittest.TestCase):
 def fixture(self):
  tmp=tempfile.TemporaryDirectory();self.addCleanup(tmp.cleanup);base=pathlib.Path(tmp.name);root=base/'goal';cycle=root/'cycles/000008';cycle.mkdir(parents=True)
  put=lambda p,v:p.write_text(json.dumps(v));manifest=base/'manifest.json';state=base/'state.json';controller=base/'controller';live=base/'live.json'
  controller.write_text('#!/usr/bin/python3\nprint(open('+repr(str(live))+').read())\n');controller.chmod(0o700)
  recovery={'stop_id':'stop8','attempt_id':'attempt8','session_id':'child8','execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}};put(live,{'state':'blocked','execution_recovery':recovery})
  out={'sequence':8,'status':'failed','failure_kind':'blocked','phase':'native','error':'Error: Operator stopping','cleanup_confirmed':True,'goal_id':'goal','job_id':'job8','attempt':'attempt8','child':'child8','parent':'parent8','worktree':str(base/'worker'),'recovery':recovery,'usage':{'cost':0,'accounting_status':'unknown'}};put(cycle/'result.json',out);put(cycle/'controller.json',{})
  history=[{'type':'assistant','content':[{'type':'tool','name':'shell','state':{'status':'completed','metadata':{'exit':1},'input':{'command':'synthetic nonzero test'}}}]}];put(cycle/'native-context-final.json',history)
  cfg=root/'supervisor.json';put(cfg,{'controller':str(controller),'controller_sha256':E.sha(controller)})
  (root/'task.txt').write_text('Preserve the task contract; repair and independently verify.')
  pause=root/'PAUSE';pause.write_text('Exact synthetic owner pause\n')
  owner=base/'stop.json';put(owner,{'action':'SIGTERM exact owned foreground dispatcher','new_permission_denial':False,'owner':{'pid':91,'start_ticks':101,'boot_id':'synthetic-boot','uid':os.getuid()}})
  receipt=base/'receipt.json';put(receipt,{'observer':'linux_subreaper_waitpid_pidfd','execution_id':'exec8','root_pid':91,'supervisor':{'start_ticks':100,'boot_id':'synthetic-boot'},'quiescent':True,'leaked_descendants':False,'cleanup_error':None,'root_exit_status':0})
  plan=base/'plan.json';put(plan,{'execution_id':'exec8','argv':['run','--manifest',str(manifest)]})
  locks=[]
  for p in [base/'pool.lock',root/'supervisor.lock']:
   f=p.open('a');fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);locks.append(f);self.addCleanup(f.close)
  m={'controller':str(controller),'pool_lock':str(base/'pool.lock'),'state':str(state),'goals':[{'id':'goal','root':str(root)}],'trusted_files':{str(p):E.sha(p) for p in [P/'owner-interruption.py',cfg,owner,receipt,plan]}}
  proof=root/M.NAME;v={'version':1,'kind':'owner-interruption','sequence':8,'original_sha256':E.sha(cycle/'result.json'),'history_sha256':E.sha(cycle/'native-context-final.json'),'identity':{k:out[k] for k in ['goal_id','job_id','attempt','child','parent','worktree']},'manifest':str(manifest),'owner_stop':str(owner),'owned_receipt':str(receipt),'owned_plan':str(plan),'stop_id':'stop8','pause_sha256':E.sha(pause)};put(proof,v);put(manifest,m)
  def seal():
   m=E.read(manifest);m['trusted_files'].pop(str(proof),None)
   for p in m['trusted_files']:m['trusted_files'][p]=E.sha(p)
   v=E.read(proof);v['manifest_without_interruption_sha256']=E.digest(m);put(proof,v);m['trusted_files'][str(proof)]=E.sha(proof);put(manifest,m);put(state,{'manifest_digest':E.digest(m)})
  seal();return locals()
 def audit(self,f,n=9):return M.audit(f['root'],f['cycle'],f['controller'],n)
 def test_real_locks_and_controller_read_preserve_failed_unknown_attempt(self):
  f=self.fixture();before=(f['cycle']/'result.json').read_bytes();r=self.audit(f);self.assertFalse(r['acceptance_granted']);self.assertEqual(r['access_grants'],[]);self.assertEqual(r['accounting'],'unknown');self.assertEqual(before,(f['cycle']/'result.json').read_bytes());self.assertTrue(f['pause'].exists())
 def test_denial_unknown_error_and_running_tool_block_even_with_fresh_owner_pin(self):
  for status,error in [('error',{'type':'permission.rejected'}),('error',{'type':'unknown'}),('running',None),('completed',{'type':'permission.rejected'})]:
   f=self.fixture();h=E.read(f['cycle']/'native-context-final.json');h[0]['content'][0]['state'].update(status=status,error=error);f['put'](f['cycle']/'native-context-final.json',h);v=E.read(f['proof']);v['history_sha256']=E.sha(f['cycle']/'native-context-final.json');f['put'](f['proof'],v);f['seal']()
   with self.assertRaisesRegex(ValueError,'remains fenced'):self.audit(f)
 def test_changed_original_history_and_owner_evidence_fail_closed(self):
  for file in ['result.json','native-context-final.json']:
   f=self.fixture();(f['cycle']/file).write_text('{}')
   with self.assertRaises(ValueError):self.audit(f)
  for field,value in [('quiescent',False),('leaked_descendants',True),('root_pid',92),('execution_id','other')]:
   f=self.fixture();r=E.read(f['receipt']);r[field]=value;f['put'](f['receipt'],r);f['seal']()
   with self.assertRaises(ValueError):self.audit(f)
 def test_other_pause_is_never_cleared(self):
  f=self.fixture();f['pause'].write_text('Human says stop')
  with self.assertRaisesRegex(ValueError,'Different explicit pause'):self.audit(f)
  self.assertEqual(f['pause'].read_text(),'Human says stop')
 def test_no_pause_is_valid_only_with_same_pinned_record(self):
  f=self.fixture();f['pause'].unlink();self.audit(f)
  m=E.read(f['manifest']);m['trusted_files'].pop(str(f['proof']));f['put'](f['manifest'],m);f['put'](f['state'],{'manifest_digest':E.digest(m)})
  with self.assertRaisesRegex(ValueError,'not owner-pinned'):self.audit(f)
 def test_live_recovery_identity_policy_and_stop_must_match(self):
  for key,value in [('stop_id','other'),('attempt_id','other'),('session_id','other'),('execution_stop',{'status':'unknown'}),('accounting',{'status':'violating'})]:
   f=self.fixture();r=E.read(f['live']);r['execution_recovery'][key]=value;f['put'](f['live'],r)
   with self.assertRaises(ValueError):self.audit(f)
 def test_locks_sequence_and_consumption_are_required(self):
  for which in [0,1]:
   f=self.fixture();fcntl.flock(f['locks'][which],fcntl.LOCK_UN)
   with self.assertRaisesRegex(ValueError,'lock'):self.audit(f)
  f=self.fixture()
  with self.assertRaisesRegex(ValueError,'Stale'):self.audit(f,10)
  n=f['root']/'cycles/000009';n.mkdir();self.audit(f);(n/'controller.json').write_text('{}')
  with self.assertRaisesRegex(ValueError,'consumed'):self.audit(f)
 def test_resume_audits_before_pause_mutation(self):
  from unittest.mock import patch
  spec=importlib.util.spec_from_file_location('resume_dispatch',P/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D)
  f=self.fixture();native={'status':'paused','sequence':8,'current':None,'last_cycle':{'result':str(f['cycle']/'result.json')}};f['put'](f['root']/'status.json',native);f['locks'][1].close();f['pause'].write_text('Changed human pause')
  with patch.object(D.subprocess,'run') as run:
   with self.assertRaisesRegex(ValueError,'Different explicit pause'):D.owner_resume(E.read(f['manifest']))
   run.assert_not_called()
  self.assertEqual(E.read(f['root']/'status.json'),native)
 def test_full_context_gate_uses_interruption_without_rewriting_history(self):
  f=self.fixture();f['pause'].unlink();repo=f['root']/'repository';repo.mkdir()
  def git(*args):return subprocess.check_output(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@localhost',*args],text=True).strip()
  git('init','-q');(repo/'code').write_text('fixture');git('add','.');git('commit','-qm','fixture');base=git('rev-parse','HEAD')
  out=E.read(f['cycle']/'result.json');out.update(ended_utc='2026-01-01T00:00:00Z',base=base);f['put'](f['cycle']/'result.json',out);v=E.read(f['proof']);v['original_sha256']=E.sha(f['cycle']/'result.json');f['put'](f['proof'],v);f['seal']()
  cfg=E.read(f['cfg']);goal={'id':'goal','objective':'useful work','completion_criteria':['verified'],'pause_conditions':['native denial']};data=f['base']/'input.json';f['put'](data,{'root':str(f['root']),'repo':str(repo),'state':{'base':base},'goal':goal,'cfg':cfg,'beforeSequence':9})
  code="import {readFileSync} from 'node:fs';import {buildResearchContext} from "+json.dumps((P/'research-context.mjs').as_uri())+";console.log(JSON.stringify(await buildResearchContext(JSON.parse(readFileSync(process.argv[1])))));"
  result=json.loads(subprocess.check_output([shutil.which('node'),'--input-type=module','-e',code,str(data)],text=True));self.assertEqual(result['provenance']['sources'][0]['permission_resolution']['kind'],'owner-interruption');self.assertEqual(E.read(f['cycle']/'result.json')['status'],'failed')
if __name__=='__main__':unittest.main(verbosity=2)
