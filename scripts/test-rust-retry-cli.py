"""Receipt-bound native retry controls with a fake read-only inspect transport.
The candidate is a real Git commit; no model or remote is contacted.
"""
import copy,hashlib,importlib.util,json,os,pathlib,subprocess,tempfile,unittest
S=pathlib.Path(__file__).resolve().parent
sp=importlib.util.spec_from_file_location('native_fixture',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(sp);sp.loader.exec_module(helper);fixture_temp,binary=helper.native_binary(S.parent)
class Retry(unittest.TestCase):
 def test_exact_failed_candidate_stop_and_pause_bind_retry_intent(self):
  with tempfile.TemporaryDirectory() as d:
   top=pathlib.Path(d);root=top/'goal';repo=root/'repository';repo.mkdir(parents=True);cycle=root/'cycles/000001';cycle.mkdir(parents=True)
   def put(p,v):p.write_text(json.dumps(v))
   def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
   def git(*args):return subprocess.check_output(['git','-C',str(repo),'-c','core.hooksPath=/dev/null','-c','user.name=Fixture','-c','user.email=fixture@invalid',*args],text=True).strip()
   git('init','-q');(repo/'product.txt').write_text('baseline');git('add','.');git('commit','-qm','baseline');base=git('rev-parse','HEAD');(repo/'product.txt').write_text('unaccepted candidate');git('commit','-qam','candidate');candidate=git('rev-parse','HEAD')
   live=top/'inspect.json';controller=top/'inspect-fixture';controller.write_text('#!/bin/sh\ncat '+str(live)+'\n');controller.chmod(0o700)
   recovery={'stop_id':'owned-stop','attempt_id':'attempt','session_id':'session','execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}
   receipt={'verification_id':'failed-receipt','candidate_commit':candidate,'outcome':'failed'};observed={'execution_recovery':recovery,'candidate_commit':candidate,'active_verification_receipt':receipt};put(live,observed)
   put(root/'supervisor.json',{'initial_base':base});put(root/'job-template.json',{'allowed_paths':['product.txt'],'protected_paths':['frozen.txt']});put(cycle/'controller.json',{})
   result={'goal_id':'goal','sequence':1,'status':'failed','cleanup_confirmed':True,'recovery':recovery,'job_id':'job','candidate':candidate,'scope_approved':True,'candidate_status':{'active_verification_receipt':receipt},'usage':{'cost':None,'tokens':None}};rp=cycle/'result.json';put(rp,result);put(root/'status.json',{'sequence':1,'current':None,'status':'paused','last_cycle':{'result':str(rp)}})
   manifest={'controller':str(controller),'trusted_files':{str(controller):sha(controller)},'state':str(top/'state.json'),'goals':[{'id':'goal','root':str(root),'repository':str(repo)}]};mp=top/'manifest.json';put(mp,manifest);queue=top/'q'
   def run():return subprocess.run([str(binary),'research-dispatch','retry','--manifest',str(mp),'--queue',str(queue),'--goal','goal'],capture_output=True,text=True,timeout=20)
   before=rp.read_bytes();first=run();self.assertEqual(first.returncode,0,first.stderr);again=run();self.assertEqual(again.stdout,first.stdout);intents=json.loads((queue/'intents.json').read_text());self.assertEqual(len(intents['retries']),1);self.assertEqual(intents['retries'][0]['evidence']['candidate'],candidate);self.assertEqual(before,rp.read_bytes());self.assertIsNone(json.loads(rp.read_text())['usage']['cost'])
   for changed in [{**observed,'execution_recovery':{**recovery,'stop_id':'foreign-stop'}},{**observed,'active_verification_receipt':{**receipt,'verification_id':'foreign-receipt'}}]:
    put(live,changed);self.assertNotEqual(run().returncode,0)
   put(live,observed);(root/'PAUSE').write_text('Unchanged explicit owner pause');self.assertNotEqual(run().returncode,0);self.assertEqual((root/'PAUSE').read_text(),'Unchanged explicit owner pause');(root/'PAUSE').unlink()
   put(rp,{**result,'fence_error':{'uncertain':True}});self.assertNotEqual(run().returncode,0);put(rp,result)
   self.assertEqual(len(json.loads((queue/'intents.json').read_text())['retries']),1)
if __name__=='__main__':unittest.main(verbosity=2)
