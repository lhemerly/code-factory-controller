"""Rust backlog CLI parity against the Python state format and controller protocol.
The HTTP controller is a deterministic fixture; no model/provider is contacted.
"""
import copy
import fcntl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER', ROOT/'target/debug/code-factory-controller')).resolve()
spec = importlib.util.spec_from_file_location('legacy_runner', ROOT/'scripts/goal-runner.py')
legacy = importlib.util.module_from_spec(spec); spec.loader.exec_module(legacy)
TOKEN = 'fixture-capability-no-external-authority'

class Backlog(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name); self.path=self.root/'state.json'; self.mp=self.root/'goal.json'
        self.jobs={}; self.calls=[]; self.mode='done'; self.errors=[]
        owner=self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                try:
                    assert self.headers['Authorization']=='Bearer '+TOKEN
                    body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    op=self.path.rsplit('/',1)[-1]; owner.calls.append((op,body))
                    if op=='list':result=list(owner.jobs.values())
                    elif op=='admit':
                        # The actual CLI must commit admission intent before HTTP.
                        assert json.loads(owner.path.read_text())['current']['input']==body
                        owner.jobs[body['job_id']]={'input':body,'state':'queued'};result={}
                    else:
                        job=owner.jobs[body['job_id']]
                        if op=='run':job['state']=owner.mode
                        elif op=='abort':job.update(state='blocked',attempt={'dispatch_state':'resolved'})
                        elif op=='requeue':job['state']='queued'
                        result={'state':job['state']}
                        if job['state']=='done':result.update(integrated_commit=owner.commit,active_verification_receipt={'outcome':'passed','candidate_commit':owner.commit},delivery={'commit':owner.commit})
                        if job['state']=='running' and owner.mode=='failed_verification':result['active_verification_receipt']={'outcome':'failed'}
                    data=json.dumps(result).encode();self.send_response(200);self.end_headers();self.wfile.write(data)
                except Exception as error:
                    owner.errors.append(repr(error));self.send_response(500);self.end_headers()
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.addCleanup(self.server.server_close);self.addCleanup(self.server.shutdown)
        self.url='http://127.0.0.1:'+str(self.server.server_port)
        self.commit='b'*40
        self.manifest={'goal_id':'approved-fixture','base_commit':'a'*40,'tasks':[{'job_id':'one'},{'job_id':'two'}],'remaining_work':['unproven broader objective']}

    def invoke(self, manifest=None, **kwargs):
        self.mp.write_text(json.dumps(manifest or self.manifest))
        run=subprocess.run([str(BINARY),'run','--manifest',str(self.mp),'--state',str(self.path),'--url',self.url,'--poll-seconds','.001',*kwargs.pop('args',[])],env={**os.environ,'FACTORY_OPERATOR_TOKEN':TOKEN},capture_output=True,text=True,timeout=kwargs.pop('timeout',10),**kwargs)
        self.assertFalse(self.errors,self.errors)
        return run

    def state(self):return json.loads(self.path.read_text())

    def test_chained_backlog_and_bidirectional_state_compatibility(self):
        # Include policy hashes which differ under ordinary compact JSON, UTF-8
        # serialization or some scientific-number formatting.
        self.manifest['tasks'][0]['fixture_policy']={'unicode':'雪 😀 \x7f','nested':{'z':1e-5,'a':[True,None,-0.0,1e16,2.5]}}
        initial=legacy.Runner(self.manifest,self.path,lambda op,body:None)
        hashes=initial.state['task_hashes']; policy=initial.state['acceptance_policy_digest']
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr)
        state=self.state();self.assertEqual(state['status'],'awaiting_plan')
        self.assertEqual(state['task_hashes'],hashes);self.assertEqual(state['acceptance_policy_digest'],policy)
        self.assertEqual(self.jobs['two']['input']['base_commit'],self.commit)
        self.assertEqual([op for op,_ in self.calls].count('admit'),2)
        reopened=legacy.Runner(self.manifest,self.path,lambda op,body:None)
        self.assertEqual(reopened.state['completed'],state['completed'])
        self.assertEqual(self.invoke().returncode,2)
        self.assertEqual([op for op,_ in self.calls].count('admit'),2)

    def test_blocked_unknown_usage_is_retained(self):
        self.mode='blocked'
        legacy.Runner(self.manifest,self.path,lambda op,body:None)
        state=self.state();state['usage']={'cost':None,'tokens':None};state['prior_denial']={'status':'failed','outcome':'not_executed'};legacy.save(self.path,state)
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr)
        state=self.state();self.assertEqual(state['status'],'needs_attention')
        self.assertEqual(state['usage'],{'cost':None,'tokens':None});self.assertEqual(state['prior_denial']['outcome'],'not_executed')
        self.assertNotIn('abort',[op for op,_ in self.calls]);self.assertNotIn('requeue',[op for op,_ in self.calls])

    def test_policy_changes_rejected_without_state_or_api_effects(self):
        legacy.Runner(self.manifest,self.path,lambda op,body:None)
        original=self.path.read_bytes()
        for mutation in [{'base_commit':'c'*40},{'goal_id':'foreign'},{'tasks':[{'job_id':'altered'}]},{'verification_environment':{'PATH':'/different'}}]:
            with self.subTest(mutation=mutation):
                run=self.invoke({**self.manifest,**mutation})
                self.assertNotEqual(run.returncode,0);self.assertEqual(self.path.read_bytes(),original);self.assertEqual(self.calls,[])

    def test_single_owner_preserves_existing_state(self):
        legacy.Runner(self.manifest,self.path,lambda op,body:None)
        original=self.path.read_bytes()
        with Path(str(self.path)+'.lock').open('a') as held:
            fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
            run=self.invoke();self.assertNotEqual(run.returncode,0);self.assertIn('another owner',run.stderr)
        self.assertEqual(self.path.read_bytes(),original);self.assertEqual(self.calls,[])

    def test_legacy_owner_on_symlink_state_alias_is_not_bypassed(self):
        legacy.Runner(self.manifest,self.path,lambda op,body:None)
        target=self.root/'canonical-state.json'
        self.path.rename(target);self.path.symlink_to(target)
        original=target.read_bytes()
        with Path(str(self.path)+'.lock').open('a') as held:
            fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
            run=self.invoke();self.assertNotEqual(run.returncode,0);self.assertIn('another owner',run.stderr)
        self.assertEqual(target.read_bytes(),original);self.assertEqual(self.calls,[])

    def test_unresolved_repair_is_fenced_without_requeue(self):
        legacy.Runner(self.manifest,self.path,lambda op,body:None)
        task={**self.manifest['tasks'][0],'base_commit':self.manifest['base_commit']}
        state=self.state();state['current']={'input':task,'reworks':0,'repair_pending':True};legacy.save(self.path,state)
        self.jobs['one']={'input':task,'state':'blocked','attempt':{'dispatch_state':'unknown'}}
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr)
        self.assertEqual(self.state()['status'],'needs_attention');self.assertNotIn('requeue',[op for op,_ in self.calls])

    def test_resolved_repair_requeues_once(self):
        legacy.Runner(self.manifest,self.path,lambda op,body:None)
        task={**self.manifest['tasks'][0],'base_commit':self.manifest['base_commit']}
        state=self.state();state['current']={'input':task,'reworks':0,'repair_pending':True};legacy.save(self.path,state)
        self.jobs['one']={'input':task,'state':'blocked','attempt':{'dispatch_state':'resolved'}}
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr)
        self.assertEqual([op for op,_ in self.calls].count('requeue'),1);self.assertNotIn('abort',[op for op,_ in self.calls])

    def test_budget_preserves_active_job(self):
        self.mode='running'
        run=self.invoke(args=['--max-seconds','1']);self.assertEqual(run.returncode,75,run.stderr)
        self.assertEqual(self.state()['status'],'budget_exhausted');self.assertEqual(self.state()['current']['input']['job_id'],'one')
        self.assertEqual([op for op,_ in self.calls].count('admit'),1)

    def test_real_acceptance_then_restart_audit_without_reexecution(self):
        repo=self.root/'repo';repo.mkdir();subprocess.run(['git','init','-q',str(repo)],check=True)
        (repo/'product.txt').write_text('accepted fixture output\n')
        subprocess.run(['git','-C',str(repo),'add','.'],check=True)
        subprocess.run(['git','-C',str(repo),'-c','user.name=Fixture','-c','user.email=fixture@invalid','commit','-qm','fixture'],check=True)
        self.commit=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip()
        count=self.root/'checks'
        command=f'test "$(cat product.txt)" = "accepted fixture output" && echo x >> {count} && printf \'{{"passed":true,"skipped":false,"executed":1,"failed":0,"commit":"%s","evidence_type":"fixture_runtime"}}\' "$FACTORY_VERIFICATION_COMMIT"'
        self.manifest.update(repository=str(repo),remaining_work=[],required_evidence=['fixture_runtime'],completion_checks=[{'id':'product','evidence_type':'fixture_runtime','argv':['/bin/sh','-c',command]}])
        run=self.invoke();self.assertEqual(run.returncode,0,run.stderr)
        state=self.state();self.assertEqual(state['status'],'completed');self.assertEqual(count.read_text(),'x\n')
        run=self.invoke();self.assertEqual(run.returncode,0,run.stderr);self.assertEqual(count.read_text(),'x\n')
        Path(state['acceptance']['receipts'][0]['stderr']).write_text('tampered')
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr);self.assertEqual(count.read_text(),'x\n')
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr);self.assertEqual(count.read_text(),'x\n')
        self.assertEqual(self.state()['status'],'needs_attention')

    def test_backlog_exhaustion_without_evidence_never_completes(self):
        self.manifest['remaining_work']=[]
        run=self.invoke();self.assertEqual(run.returncode,2,run.stderr);self.assertEqual(self.state()['status'],'awaiting_acceptance')

    def test_literal_loopback_and_positive_budget_required_before_effects(self):
        for origin in ['http://localhost:1','http://127.1:1','http://2130706433:1','http://127.0.0.1:1/','http://127.0.0.1:1?x','http://user@127.0.0.1:1']:
            with self.subTest(origin=origin):
                self.url=origin;run=self.invoke();self.assertNotEqual(run.returncode,0);self.assertFalse(self.path.exists())

if __name__=='__main__':unittest.main(verbosity=2)
