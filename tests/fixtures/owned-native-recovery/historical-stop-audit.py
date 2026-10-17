import importlib.util,pathlib,json,unittest
S=pathlib.Path.cwd()/'scripts';sp=importlib.util.spec_from_file_location('frozen_base',S/'host-base-dispatch.py');F=importlib.util.module_from_spec(sp);sp.loader.exec_module(F);D=F.D
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
