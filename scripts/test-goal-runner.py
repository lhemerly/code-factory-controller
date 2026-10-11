import importlib.util,json,pathlib,tempfile,unittest
s=importlib.util.spec_from_file_location('runner',pathlib.Path(__file__).with_name('goal-runner.py'))
m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Tests(unittest.TestCase):
 def test_resume_and_chained_base(self):
  jobs={};calls=[]
  def api(op,body):
   calls.append(op)
   if op=='list':return list(jobs.values())
   if op=='admit':jobs[body['job_id']]={'input':body,'state':'queued'};return {}
   j=jobs[body['job_id']]
   if op=='run':j['state']='done'
   if j['state']=='done':return {'state':'done','integrated_commit':'b'*40,'active_verification_receipt':{'outcome':'passed','candidate_commit':'b'*40},'delivery':{'commit':'b'*40}}
   return {'state':j['state'],'active_verification_receipt':None}
  with tempfile.TemporaryDirectory() as d:
   path=pathlib.Path(d)/'state.json';manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'},{'job_id':'two'}],'remaining_work':['broader runtime']}
   r=m.Runner(manifest,path,api);self.assertTrue(r.step())
   r=m.Runner(manifest,path,api);self.assertTrue(r.step());self.assertTrue(r.step())
   self.assertEqual(jobs['two']['input']['base_commit'],'b'*40)
   self.assertTrue(r.step());self.assertFalse(r.step());self.assertEqual(r.state['status'],'awaiting_plan')
   self.assertEqual(calls.count('admit'),2)
   manifest['tasks'][0]['job_id']='altered'
   with self.assertRaises(ValueError):m.Runner(manifest,path,api)
 def test_false_done_rejected(self):
  def api(op,b):
   if op=='list':return [{'input':{'job_id':'one','base_commit':'a'*40}}]
   return {'state':'done','integrated_commit':'b'*40,'active_verification_receipt':{'outcome':'passed','candidate_commit':'c'*40},'delivery':{'commit':'b'*40}}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]},pathlib.Path(d)/'state.json',api)
   with self.assertRaises(ValueError):r.step()
 def test_blocked_fenced(self):
  def api(op,b):
   if op=='list':return [{'input':{'job_id':'one','base_commit':'a'*40}}]
   return {'state':'blocked','blockers':['outcome_unknown']}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]},pathlib.Path(d)/'state.json',api)
   self.assertFalse(r.step());self.assertEqual(r.state['status'],'needs_attention')
 def test_failed_verification_resolves_before_requeue(self):
  calls=[]
  task={'job_id':'one','base_commit':'a'*40}
  def api(op,b):
   calls.append(op)
   if op=='list':return [{'input':task}]
   if op=='status':return {'state':'running','active_verification_receipt':{'outcome':'failed'}}
   return {}
  with tempfile.TemporaryDirectory() as d:
   r=m.Runner({'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}],'max_reworks':1},pathlib.Path(d)/'state.json',api)
   self.assertTrue(r.step());self.assertLess(calls.index('abort'),calls.index('requeue'))
   self.assertFalse(r.step());self.assertEqual(calls.count('abort'),1)
 def test_resume_after_abort_before_requeue(self):
  task={'job_id':'one','base_commit':'a'*40};calls=[]
  def api(op,b):
   calls.append(op)
   if op=='list':return [{'input':task,'attempt':{'dispatch_state':'resolved'}}]
   if op=='status':return {'state':'blocked'}
   return {}
  with tempfile.TemporaryDirectory() as d:
   path=pathlib.Path(d)/'state.json';manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]}
   r=m.Runner(manifest,path,api);r.state['current']={'input':task,'reworks':0,'repair_pending':True};r.persist()
   r=m.Runner(manifest,path,api);self.assertTrue(r.step());self.assertIn('requeue',calls);self.assertNotIn('abort',calls)
 def test_completed_delivery_reaudited_on_restart(self):
  def api(op,b):return {'state':'blocked','integrated_commit':'b'*40,'delivery':{'commit':'b'*40}}
  with tempfile.TemporaryDirectory() as d:
   path=pathlib.Path(d)/'state.json';manifest={'goal_id':'g','base_commit':'a'*40,'tasks':[{'job_id':'one'}]}
   r=m.Runner(manifest,path,api);r.state['completed']=[{'job_id':'one','integrated_commit':'b'*40,'delivery':{'commit':'b'*40}}];r.persist()
   r=m.Runner(manifest,path,api);self.assertFalse(r.step());self.assertEqual(r.state['audit_failure'],'one')
if __name__=='__main__':unittest.main()
