#!/usr/bin/env python3
"""Test-only deterministic provider using a real controller/verifier. No models."""
import hashlib,json,os,pathlib,secrets,socket,subprocess,sys,time,urllib.request
root=pathlib.Path(sys.argv[1]);repo=root/'repository';binary=os.environ['FACTORY_FIXTURE_CONTROLLER'];goal=json.loads((root/'goal.json').read_text());state=json.loads((root/'status.json').read_text()) if (root/'status.json').exists() else {'sequence':0};seq=state['sequence']+1;directory=root/'cycles'/str(seq);directory.mkdir(parents=True);operator=secrets.token_hex(32);worker=secrets.token_hex(32);job='fixture-'+str(seq)
def git(*a):return subprocess.check_output(['git','-c','core.hooksPath=/dev/null',*a],cwd=repo,env=env,text=True).strip()
env={**os.environ,'GIT_AUTHOR_NAME':'Fixture','GIT_AUTHOR_EMAIL':'fixture@localhost','GIT_COMMITTER_NAME':'Fixture','GIT_COMMITTER_EMAIL':'fixture@localhost'}
base=git('rev-parse','HEAD');s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close();url='http://127.0.0.1:'+str(port)
config={'project_id':goal['id'],'repository':str(repo),'data_dir':str(directory/'controller'),'delivery_dir':str(directory/'delivery'),'integration_ref':'refs/heads/fixture-integration','profiles':[{'id':'fixture','version':'1','timeout_seconds':30,'checks':[{'id':'result','argv':['/usr/bin/python3','-c',"import pathlib;assert pathlib.Path('result.txt').read_text()=='verified result\\n'"]}]}],'policy':{'models':['fixture/free'],'free_only':True,'spend_ceiling':0,'data_approved':True,'cooldown_seconds':1},'max_workers':1,'lease_seconds':3600,'max_attempts':1,'opencode_url':None,'verifier_executable':binary};(directory/'controller.json').write_text(json.dumps(config))
p=subprocess.Popen([binary,'serve','--config',str(directory/'controller.json'),'--listen','127.0.0.1:'+str(port)],env={**env,'FACTORY_OPERATOR_TOKEN':operator},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def call(path,body,token):
 req=urllib.request.Request(url+path,data=json.dumps(body).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
 with urllib.request.urlopen(req,timeout=15) as response:return json.load(response)
def op(name,**body):return call('/operator/'+name,{'job_id':job,**body},operator)
def request(name,key,**body):return call('/v1/'+name,{'protocol_version':'0.1','request_id':hashlib.sha256(json.dumps(['0.1',goal['id'],'fixture-session',name,key],separators=(',',':')).encode()).hexdigest(),'project_id':goal['id'],'session_id':'fixture-session','idempotency_key':key,**body},worker)
try:
 for _ in range(100):
  try:urllib.request.urlopen(url+'/health',timeout=.1).close();break
  except Exception:time.sleep(.03)
 op('standing-goal',goal=goal)
 op('admit',base_commit=base,task_brief='Deterministic fake provider only',acceptance_criteria=[{'behavior':'Fixture artifact','check_ids':['result'],'expected':'Exact fixture text'}],allowed_paths=['result.txt'],protected_paths=['README.md'],verification_profile_id='fixture',policy_version='fixture')
 prepared=op('prepare',mode='manual');attempt=prepared['attempt'];work=pathlib.Path(attempt['worktree']);op('bind',session_id='fixture-session',worker_token=worker)
 response=request('factory_next_task','claim',adapter_capabilities=['opencode']);assert not response.get('error'),response
 (work/'result.txt').write_text('verified result\n');subprocess.run(['git','-C',str(work),'-c','core.hooksPath=/dev/null','add','result.txt'],check=True,env=env);subprocess.run(['git','-C',str(work),'-c','core.hooksPath=/dev/null','commit','-qm','fake provider result'],check=True,env=env);candidate=subprocess.check_output(['git','-C',str(work),'rev-parse','HEAD'],text=True).strip()
 body={'job_id':job,'attempt_id':attempt['attempt_id'],'lease_id':attempt['lease_id'],'candidate_commit':candidate}
 response=request('factory_request_verification','verify',**body,verification_profile_id='fixture');assert not response.get('error'),response
 op('advance');verified=op('status');receipt=verified['active_verification_receipt'];assert receipt['outcome']=='passed',verified
 response=request('factory_submit_candidate','submit',**body,verification_id=receipt['verification_id'],artifact_manifest=receipt['artifacts'],change_summary='Fake provider fixture only');assert not response.get('error'),response
 op('advance');done=op('status');assert done['state']=='done',done
finally:
 p.terminate();p.wait(timeout=10)
out={'goal_id':goal['id'],'sequence':seq,'status':'accepted','cleanup_confirmed':True,'job_id':job,'acceptance':done,'model':'fixture/free','usage':{'source':'fake provider fixture','cost':None,'tokens':{'input':None,'output':None,'cache':{'read':None,'write':None}}},'worktree':str(work)};(directory/'result.json').write_text(json.dumps(out));(root/'status.json').write_text(json.dumps({'sequence':seq,'current':None,'status':'waiting','last_cycle':{'sequence':seq,'result':str(directory/'result.json')}}));print('fake-provider result with real independent controller receipt',goal['id'])
