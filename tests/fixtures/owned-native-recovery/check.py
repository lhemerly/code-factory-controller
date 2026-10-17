"""Owner-frozen worker regression. Run from the assigned repository root.

Only fixture setup is new; Cases below is the historical independent regression.
Native/provider/helper effects are injected, never executed.
"""
import importlib.util,json,pathlib,tempfile,unittest

S=pathlib.Path.cwd()/'scripts'
sp=importlib.util.spec_from_file_location('dispatch_under_test',S/'research-dispatch.py')
D=importlib.util.module_from_spec(sp);sp.loader.exec_module(D)

class Fixture(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
  self.root=pathlib.Path(self.tmp.name);self.clock=0;self.calls=[];self.goals=[]
  for name in ['scroll','ink']:
   root=self.root/name;repo=root/'repository';repo.mkdir(parents=True)
   self.goals.append({'id':name,'root':str(root),'repository':str(repo),'cycle_command':['/usr/bin/python3','/fake-cycle',str(root),'--one-cycle'],'between_cycles_seconds':0,'acceptance':{'completion_checks':[{'id':'fixture','evidence_type':'scientific_goal','argv':['/usr/bin/true']}],'required_evidence':['scientific_goal']}})
  self.m={'goals':self.goals,'worker_slots':1,'pool_lock':str(self.root/'pool.lock'),'controller':'/fake-controller','state':str(self.root/'state.json')}
  self.path=self.root/'state.json'
 def fake(self,goal,record,status='accepted'):
  self.calls.append(goal['id']);root=pathlib.Path(goal['root']);seq=record['expected_sequence'];result=root/('result-'+str(seq)+'.json')
  out={'goal_id':goal['id'],'sequence':seq,'status':status,'cleanup_confirmed':True,'model':'opencode/big-pickle','job_id':goal['id']+'-'+str(seq),'usage':{'cost':0,'tokens':{'input':9,'output':2,'cache':{'read':17,'write':None}}},'recovery':{'execution_stop':{'status':'confirmed'},'accounting':{'status':'unknown'}}}
  result.write_text(json.dumps(out));D.save(root/'status.json',{'sequence':seq,'current':None,'status':'waiting','last_cycle':{'sequence':seq,'result':str(result)}})
 def manager(self,**kwargs):
  return D.Dispatcher(self.m,self.path,runner=kwargs.pop('runner'),audit=kwargs.pop('audit'),complete=lambda *_:{'passed':False,'status':'acceptance_failed'},clock=lambda:self.clock,**kwargs)

class F:
 Fixture=Fixture

class Cases(unittest.TestCase):
 def test_confirmed_denial_must_finish_audit_and_remain_paused(self):
  f=F.Fixture();f.setUp();self.addCleanup(f.doCleanups);f.goals=f.goals[:1];f.m['goals']=f.goals;audits=[]
  def runner(g,record):
   f.fake(g,record,status='failed');r=pathlib.Path(g['root']);s=D.queue_api.read(r/'status.json');p=pathlib.Path(s['last_cycle']['result']);x=D.queue_api.read(p);x.update(phase='native',failure_kind='blocked',error='Permission denied: external_directory',fence_error=None);D.save(p,x);s.update(status='paused',pause_reason=x['error']);D.save(r/'status.json',s)
  def audit(g,out):audits.append(out);return None
  d=f.manager(runner=runner,audit=audit);d.step()
  self.assertEqual(len(audits),1,'Every confirmed failed attempt still requires independent controller audit')
  self.assertEqual(d.s['goals']['scroll']['state'],'paused','A positively stopped permission rejection must not allow another worker')
if __name__=='__main__':unittest.main()
