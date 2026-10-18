"""Regenerate synthetic Python dashboard oracles; contains no runtime/user state."""
import copy,hashlib,json,pathlib,sys
ROOT=pathlib.Path(__file__).resolve().parents[1];sys.path.insert(0,str(ROOT))
from dashboard import domain
identity={'sequence':1,'ended_utc':'2026-01-01T12:01:00Z','job_id':'fixture-job','goal_id':'fixture-goal','attempt':'fixture-attempt','session':'fixture-session','child':'fixture-child','parent':'fixture-parent','worktree':'/fixture/work','base':'a'*40,'candidate':None,'status':'failed','cleanup_confirmed':True,'fence_error':'prior uncertainty'}
usage={'input':10,'output':20,'reasoning':30,'cache':{'read':40,'write':50}}
rec={**identity,'fence_error':None,'prior_fence_error':identity['fence_error'],'execution_stop':'confirmed','reconciliation':{'original_result':'result.json','original_result_sha256':'f'*64},'recovery':{'attempt_id':identity['attempt'],'session_id':identity['child'],'stop_id':'fixture-stop','execution_stop':{'status':'confirmed'},'accounting':{'status':'settled_zero','observations':[{'source':'session','id':identity['child'],'cost':0,'tokens':usage}]}}}
cycle={'rel':'one/cycles/000001','dir':'000001','result_sha256':'f'*64,'files':{'result.json':identity,'reconciled-result.json':rec,'native-session-final.json':{'tokens':{'input':1,'output':2,'reasoning':3,'cache':{'read':4,'write':5}},'cost':0}}}
cases=[]
def add(name,cycles):cases.append({'id':name,'cycles':cycles,'expected':domain.build_tokens(cycles)})
add('valid-reconciled-single-session',[cycle]);add('missing-usage',[])
for k in domain.RECONCILED_IDENTITY_FIELDS:
 c=copy.deepcopy(cycle);c['files']['reconciled-result.json'][k]='changed';add('changed-'+k,[c])
for field in domain.REQUIRED_IDENTITY_FIELDS:
 for missing in [None,'']:
  c=copy.deepcopy(cycle)
  for name in ['result.json','reconciled-result.json']:c['files'][name][field]=missing
  add('anonymous-'+field+'-'+str(missing),[c])
for field in ['input','output','reasoning','cache.read','cache.write']:
 for value in [None,False,-1,1.5,'10']:
  c=copy.deepcopy(cycle);t=c['files']['reconciled-result.json']['recovery']['accounting']['observations'][0]['tokens'];parts=field.split('.');target=t if len(parts)==1 else t['cache'];target[parts[-1]]=value;add('invalid-'+field+'-'+str(value),[c])
for field,value in [('original_result','../other/result.json'),('original_result','/source/other/cycles/000001/result.json'),('original_result_sha256','0'*64)]:
 c=copy.deepcopy(cycle);c['files']['reconciled-result.json']['reconciliation'][field]=value;add('provenance-'+value,[c])
for state in ['settled_zero','settled','unknown',None]:
 for cost in [None,0,0.125]:
  c={'files':{'result.json':{'usage':{'tokens':usage,'cost':cost,'accounting_status':state}}}};add('reported-'+str(state)+'-'+str(cost),[c,{'files':{}}])
path=ROOT/'tests/fixtures/native-dashboard-policy.json';path.write_text(json.dumps({'test_only':True,'oracle':'frozen Python domain.build_tokens; synthetic values only','cases':cases},indent=2)+'\n');print(len(cases),'cases',path)
