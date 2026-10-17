import importlib.util,json,os,pathlib,signal,subprocess,sys,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('handoff',ROOT/'queue-handoff.py');H=importlib.util.module_from_spec(spec);spec.loader.exec_module(H)
class Tests(unittest.TestCase):
 def test_idle_coordinator_retires_after_identity_and_state_checks(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=pathlib.Path(tmp);ready=root/'ready';state=root/'state.json';state.write_text(json.dumps({'status':'paused','current':None}));code='import pathlib,signal,time,sys;signal.signal(signal.SIGTERM,lambda *a:sys.exit(0));pathlib.Path(sys.argv[1]).write_text("ready");time.sleep(20)'
   p=subprocess.Popen([sys.executable,'-c',code,str(ready)])
   try:
    limit=time.monotonic()+3
    while not ready.exists() and time.monotonic()<limit:time.sleep(.01)
    self.assertTrue(ready.exists());record=H.D.identity(p.pid);H.stopped_owner(record,state);self.assertEqual(p.wait(timeout=2),0)
   finally:
    if p.poll() is None:p.kill();p.wait()
 def test_active_or_racing_admission_is_resumed_not_terminated(self):
  with tempfile.TemporaryDirectory() as tmp:
   state=pathlib.Path(tmp)/'state.json';state.write_text(json.dumps({'status':'running','current':{'goal':'healthy'}}));p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(20)'])
   try:
    record=H.D.identity(p.pid)
    with self.assertRaisesRegex(RuntimeError,'advanced'):H.stopped_owner(record,state)
    self.assertIsNone(p.poll());self.assertNotIn('T',(pathlib.Path('/proc')/str(p.pid)/'stat').read_text().rsplit(')',1)[1].split()[0])
   finally:p.terminate();p.wait(timeout=2)
 def test_reused_pid_identity_is_never_signalled(self):
  p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(20)'])
  try:
   record=H.D.identity(p.pid);record['start_ticks']+=1;H.stopped_owner(record,'/not-read');self.assertIsNone(p.poll())
  finally:p.terminate();p.wait(timeout=2)
 def test_cooperative_handoff_waits_for_child_completion_and_releases_only_own_pauses(self):
  with tempfile.TemporaryDirectory(prefix='handoff-cli-') as tmp:
   root=pathlib.Path(tmp);goalroot=root/'goal';repo=goalroot/'repository';repo.mkdir(parents=True);worker=root/'worker.py'
   worker.write_text("import pathlib,time,json,sys\nr=pathlib.Path(sys.argv[1]);(r/'ready').write_text('ready')\nwhile not (r/'release').exists():time.sleep(.01)\n(r/'finished').write_text('natural completion')\n(r/'status.json').write_text(json.dumps({'sequence':0,'current':None,'status':'paused'}))\n")
   state=root/'state.json';manifest=root/'manifest.json';queue=root/'q';goal={'id':'healthy','root':str(goalroot),'repository':str(repo),'cycle_command':[sys.executable,str(worker),str(goalroot),'--one-cycle'],'acceptance':{'completion_checks':[{'id':'fixture','evidence_type':'fixture','argv':['/usr/bin/true']}],'required_evidence':['fixture']},'between_cycles_seconds':0}
   m={'goals':[goal],'state':str(state),'pool_lock':str(root/'pool.lock'),'worker_slots':1,'controller':'/unused','trusted_files':{str(worker):H.D.queue_api.sha(worker)}};H.D.save(goalroot/'status.json',{'sequence':0,'current':None,'status':'waiting'})
   manual=root/'manual';(manual/'repository').mkdir(parents=True);(manual/'PAUSE').write_text('User pause');H.D.save(manual/'status.json',{'sequence':0,'current':None,'status':'paused'})
   held_goal={**goal,'id':'manual','root':str(manual),'repository':str(manual/'repository')};m['goals'].append(held_goal);H.D.save(manifest,m);initial=H.D.Dispatcher(m,state);initial.s['goals']['manual']['state']='paused';initial.persist()
   owner=subprocess.Popen([sys.executable,str(ROOT/'research-dispatch.py'),'run','--manifest',str(manifest),'--queue',str(queue)],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
   replacement=root/'replacement.py';marker=root/'replacement-started';replacement.write_text('import pathlib;pathlib.Path('+repr(str(marker))+').write_text("replacement")');pins=root/'runtime.json';H.D.save(pins,{'dispatcher':str(replacement),'files':{str(replacement):H.D.queue_api.sha(replacement)}})
   helper=None
   def wait_for(predicate):
    limit=time.monotonic()+5
    while time.monotonic()<limit:
     if predicate():return
     time.sleep(.01)
    self.fail('handoff fixture event timeout')
   try:
    wait_for(lambda:(goalroot/'ready').exists());helper=subprocess.Popen([sys.executable,str(ROOT/'queue-handoff.py'),'--manifest',str(manifest),'--queue',str(queue),'--runtime-pins',str(pins)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    wait_for(lambda:(goalroot/'PAUSE').exists());self.assertIsNone(owner.poll());self.assertFalse(marker.exists());self.assertFalse((goalroot/'finished').exists())
    (goalroot/'release').write_text('complete');stdout,stderr=helper.communicate(timeout=8);self.assertEqual(helper.returncode,0,(stdout,stderr));owner.communicate(timeout=2);self.assertEqual(owner.returncode,0);self.assertTrue(marker.exists());self.assertTrue((goalroot/'finished').exists());self.assertFalse((goalroot/'PAUSE').exists());self.assertEqual((manual/'PAUSE').read_text(),'User pause');self.assertEqual(json.loads(state.read_text())['goals']['manual']['state'],'paused')
   finally:
    for p in [helper,owner]:
     if p and p.poll() is None:p.kill();p.wait()
if __name__=='__main__':unittest.main(verbosity=2)
