import json, pathlib, subprocess, tempfile, unittest, fcntl
SCRIPT=pathlib.Path(__file__).with_name('spark-control.py')
class Controls(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=pathlib.Path(self.tmp.name)
  self.state={'status':'paused','current':None,'circuits':{'opencode/x-free':{'kind':'denied','until':99999999}}};self.write()
 def write(self):(self.root/'status.json').write_text(json.dumps(self.state))
 def run_control(self,*args):return subprocess.run(['/usr/bin/python3',str(SCRIPT),*args,'--root',str(self.root)],capture_output=True,text=True)
 def test_resume_requires_reason_and_preserves_denial(self):
  self.assertNotEqual(self.run_control('resume').returncode,0)
  self.assertEqual(self.run_control('resume','--reason','inspected confirmed stop').returncode,0)
  saved=json.loads((self.root/'status.json').read_text());self.assertEqual(saved['circuits'],self.state['circuits']);self.assertEqual(saved['status'],'waiting')
 def test_resume_rejects_unknown_current_and_live_owner(self):
  self.state['current']={'directory':'unconfirmed'};self.write();self.assertNotEqual(self.run_control('resume','--reason','retry').returncode,0)
  self.state['current']=None;self.write()
  with (self.root/'supervisor.lock').open('a') as lock:
   fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB);self.assertNotEqual(self.run_control('resume','--reason','retry').returncode,0)
 def test_pause_is_durable_and_does_not_rewrite_receipts_or_state(self):
  before=(self.root/'status.json').read_bytes();self.assertEqual(self.run_control('pause').returncode,0);self.assertTrue((self.root/'PAUSE').exists());self.assertEqual((self.root/'status.json').read_bytes(),before)
if __name__=='__main__':unittest.main(verbosity=2)
