import pathlib,tempfile,fcntl,os,subprocess,json,unittest
import importlib.util
P=pathlib.Path(__file__).resolve().parent/'permission-block-resolution.py';spec=importlib.util.spec_from_file_location('lock_resolution',P);M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M);corroborate=M.owner_lock
class LockRegression(unittest.TestCase):
 def fixture(self,parent=None):
  f=tempfile.NamedTemporaryFile(dir=parent,prefix='lock-regression-');self.addCleanup(f.close);return f,pathlib.Path(f.name)
 def test_workspace_and_tmp_kernel_identity(self):
  rows=[]
  for parent in [pathlib.Path(__file__).parent,pathlib.Path('/tmp')]:
   f,p=self.fixture(parent);fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);row=corroborate(p);self.assertEqual(row['pid'],os.getpid());rows.append(row)
  self.assertTrue(all(r['inode']>0 for r in rows))
 def test_unlocked_shared_wrong_file_and_symlink_rejected(self):
  f,p=self.fixture();g,q=self.fixture();fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
  with self.assertRaises(ValueError):corroborate(q)
  fcntl.flock(f,fcntl.LOCK_SH)
  with self.assertRaises(ValueError):corroborate(p)
  fcntl.flock(f,fcntl.LOCK_UN)
  with self.assertRaises(ValueError):corroborate(p)
  link=p.with_name(p.name+'-link');link.symlink_to(p);self.addCleanup(link.unlink)
  with self.assertRaises(ValueError):corroborate(link)
 def test_foreign_child_owner_rejected(self):
  f,p=self.fixture();process=subprocess.Popen(['/usr/bin/python3','-c','import fcntl,sys;f=open(sys.argv[1],"a");fcntl.flock(f,fcntl.LOCK_EX);print("locked",flush=True);sys.stdin.read()',str(p)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True);self.addCleanup(process.wait);self.addCleanup(process.stdout.close);self.addCleanup(process.stdin.close);self.assertEqual(process.stdout.readline().strip(),'locked')
  with self.assertRaises(ValueError):corroborate(p)
 def test_ancestor_lock_accepted_from_child(self):
  f,p=self.fixture(pathlib.Path(__file__).parent);fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);code='import importlib.util,sys,json;s=importlib.util.spec_from_file_location("resolution",sys.argv[2]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);print(json.dumps(m.owner_lock(sys.argv[1])))';r=json.loads(subprocess.check_output(['/usr/bin/python3','-c',code,str(p),str(P)],cwd=pathlib.Path(__file__).parent,text=True));self.assertEqual(r['pid'],os.getpid())
if __name__=='__main__':unittest.main(verbosity=2)
