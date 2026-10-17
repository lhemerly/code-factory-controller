import copy,importlib.util,json,pathlib,subprocess,tempfile,unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('mainpub',pathlib.Path(__file__).with_name('main-publication.py'));M=importlib.util.module_from_spec(spec);spec.loader.exec_module(M)
class Remote:
 def __init__(self,b,head):self.b=b;self.head=head;self.pushes=[];self.fail=None;self.checks=[];self.main_checks=[]
 def snapshot(self,b,commit=None):return {'repository':{'id':b['repository_id'],'full_name':b['repository'],'private':True,'default_branch':'main'},'head':self.head,'checked_commit':commit or self.head,'checks':copy.deepcopy(self.checks),'main_checks':copy.deepcopy(self.main_checks)}
 def push(self,b,repo,commit):
  self.pushes.append(commit)
  if self.fail=='before':raise RuntimeError('lost before push')
  self.head=commit
  if self.fail=='after':raise RuntimeError('lost response')
class Tests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.root=pathlib.Path(self.tmp.name);self.repo=self.root/'repo';self.repo.mkdir()
  self.git('init','-q');self.git('config','user.name','Fixture');self.git('config','user.email','fixture@localhost');(self.repo/'src').mkdir();(self.repo/'src/product.py').write_text('print(1)\n');self.git('add','.');self.git('commit','-qm','base');self.base=self.git('rev-parse','HEAD');(self.repo/'src/product.py').write_text('print(2)\n');self.git('commit','-qam','product');self.commit=self.git('rev-parse','HEAD')
  self.b={'repository':'lhemerly/ink-detector','repository_id':1422424015,'private':True,'branch':'main','remote_url':'git@github.com:lhemerly/ink-detector.git','initial_head':self.base,'goal_id':'goal','goal_root':str(self.root),'allowed_paths':['src'],'protected_paths':['src/frozen.py'],'forbidden_patterns':['private-runtime-value'],'required_checks':[]}
  self.remote=Remote(self.b,self.base);self.p=M.Publisher(self.root/'state',self.remote);self.receipt={'job':'job','commit':self.commit,'receipt':'controller-receipt','result':'fixture','result_sha256':'abc'};self.mock=patch.object(self.p,'verified',return_value=(self.repo,self.commit,self.receipt));self.mock.start()
 def tearDown(self):self.mock.stop();self.tmp.cleanup()
 def git(self,*args):return M.git(self.repo,*args)
 def publish(self):return self.p.publish(self.b,{'id':'goal'})
 def test_success_and_restart_replay_one_push(self):
  self.assertEqual(self.publish()['state'],'confirmed');self.publish();self.assertEqual(self.remote.pushes,[self.commit]);self.assertEqual(len(M.read(self.p.root/'state.json')['history']),1)
 def test_lost_response_reconciles_same_commit_without_second_push(self):
  self.remote.fail='after'
  with self.assertRaises(RuntimeError):self.publish()
  self.remote.fail=None;self.publish();self.assertEqual(len(self.remote.pushes),1)
 def test_unknown_failed_push_never_repeated(self):
  self.remote.fail='before'
  with self.assertRaises(RuntimeError):self.publish()
  self.remote.fail=None
  with self.assertRaises(ValueError):self.publish()
  self.assertEqual(len(self.remote.pushes),1)
 def test_no_receipt_no_remote_write(self):
  with patch.object(self.p,'verified',side_effect=ValueError('forged receipt')),self.assertRaises(ValueError):self.publish()
  self.assertEqual(self.remote.pushes,[])
 def test_identity_scope_visibility_branch_and_remote_cannot_expand(self):
  for key,value in [('repository','other/repo'),('repository_id',99),('private',False),('branch','dev'),('remote_url','ssh://untrusted')]:
   b={**self.b,key:value}
   with self.assertRaises(ValueError):self.p.publish(b,{'id':'goal'})
  self.assertEqual(self.remote.pushes,[])
 def test_scope_protected_content_and_binary_rejections(self):
  for override in [{'allowed_paths':['docs']},{'protected_paths':['src']},{'forbidden_patterns':['print']}]:
   with self.assertRaises(ValueError):self.p.publish({**self.b,**override},{'id':'goal'})
  self.assertEqual(self.remote.pushes,[])
 def test_real_git_text_source_naming_binary_markers_admitted_real_binary_refused(self):
  # Genuine text source may name the publisher's binary markers as literals.
  (self.repo/'src/detectors.py').write_text("binary=any(m in patch for m in (b'GIT binary patch',b'Binary files '))\n")
  self.git('add','.');self.git('commit','-qm','text source names binary markers');text=self.git('rev-parse','HEAD')
  self.assertRegex(self.p.content(self.b,self.repo,self.base,text),r'^[0-9a-f]{64}$')
  # An actually binary real-Git diff stays refused by the text-scope gate.
  (self.repo/'src/payload.bin').write_bytes(b'\0binary\xff\0payload')
  self.git('add','.');self.git('commit','-qm','actually binary payload');binary=self.git('rev-parse','HEAD')
  with self.assertRaisesRegex(ValueError,'text scope'):self.p.content(self.b,self.repo,text,binary)
  self.assertEqual(self.remote.pushes,[])
 def test_divergent_main_never_overwritten(self):
  self.git('checkout','-q','--detach',self.base);(self.repo/'different').write_text('other');self.git('add','.');self.git('commit','-qm','other');self.remote.head=self.git('rev-parse','HEAD')
  with self.assertRaises(subprocess.CalledProcessError):self.publish()
  self.assertEqual(self.remote.pushes,[])
 def test_missing_or_pending_pre_main_required_ci_fences_without_push(self):
  self.b['required_checks']=['controller']
  # Before any main push, missing, unfinished, failed and skipped required
  # checks are a hard fence: raise and cause no push.
  for checks in [[],[{'name':'controller','status':'pending','conclusion':None}],[{'name':'controller','status':'in_progress','conclusion':None}],[{'name':'controller','status':'completed','conclusion':'failure'}],[{'name':'controller','status':'completed','conclusion':'skipped'}],[{'name':'controller','status':'completed','conclusion':'neutral'}],[{'name':'other','status':'completed','conclusion':'failure'}]]:
   self.remote.checks=checks
   with self.assertRaises(ValueError):self.publish()
  self.assertEqual(self.remote.pushes,[])
  self.remote.checks=[{'name':'controller','status':'completed','conclusion':'success'}];self.assertEqual(self.publish()['state'],'confirmed')
  self.assertEqual(self.remote.pushes,[self.commit])
 def test_ci_after_push_pending_returns_awaiting_and_reconciles_once(self):
  original=self.remote.push
  def push(*args):original(*args);self.remote.checks=[{'name':'ci','status':'pending','conclusion':None}]
  with patch.object(self.remote,'push',side_effect=push):
   first=self.publish()
  self.assertEqual(first['state'],'awaiting_ci');self.assertEqual(len(self.remote.pushes),1)
  self.assertEqual(M.read(self.p.root/'state.json')['intent']['state'],'remote_observed')
  # A restarted publisher resumes the same durable intent without a second push.
  self.p=M.Publisher(self.root/'state',self.remote)
  self.remote.checks=[{'name':'ci','status':'completed','conclusion':'success'}]
  with patch.object(self.p,'verified',return_value=(self.repo,self.commit,self.receipt)):
   self.assertEqual(self.p.publish(self.b,{'id':'goal'})['state'],'confirmed')
  self.assertEqual(len(self.remote.pushes),1);self.assertEqual(len(M.read(self.p.root/'state.json')['history']),1)
 def test_confirmed_publication_replay_waits_for_ci_without_repush_or_second_history(self):
  self.assertEqual(self.publish()['state'],'confirmed')
  self.remote.checks=[{'name':'ci','status':'pending','conclusion':None}]
  self.assertEqual(self.publish()['state'],'awaiting_ci')
  self.assertEqual(len(self.remote.pushes),1);self.assertEqual(len(M.read(self.p.root/'state.json')['history']),1)
  self.remote.checks=[{'name':'ci','status':'completed','conclusion':'success'}]
  self.assertEqual(self.publish()['state'],'confirmed')
  self.assertEqual(len(self.remote.pushes),1);self.assertEqual(len(M.read(self.p.root/'state.json')['history']),1)
 def test_lost_response_reconciles_then_waits_for_pending_ci(self):
  self.remote.fail='after'
  with self.assertRaises(RuntimeError):self.publish()
  self.remote.fail=None;self.remote.checks=[{'name':'ci','status':'pending','conclusion':None}]
  self.assertEqual(self.publish()['state'],'awaiting_ci');self.assertEqual(len(self.remote.pushes),1)
  self.remote.checks=[{'name':'ci','status':'completed','conclusion':'success'}]
  self.assertEqual(self.publish()['state'],'confirmed');self.assertEqual(len(self.remote.pushes),1)
  self.assertEqual(len(M.read(self.p.root/'state.json')['history']),1)
 def test_immutable_binding_and_changed_published_head_refused(self):
  self.publish()
  with self.assertRaises(ValueError):self.p.publish({**self.b,'goal_id':'other'},{'id':'other'})
  self.remote.head=self.base
  with self.assertRaises(ValueError):self.publish()
 def test_local_bare_remote_honors_normal_non_force_push(self):
  remote=self.root/'remote.git';subprocess.run(['git','init','--bare','-q',str(remote)],check=True);self.git('push',str(remote),self.base+':refs/heads/main')
  # This is a real local Git transport only; GitHub and inference are not involved.
  self.git('push',str(remote),self.commit+':refs/heads/main');self.assertEqual(M.git(remote,'rev-parse','main'),self.commit)
  with self.assertRaises(subprocess.CalledProcessError):self.git('push',str(remote),self.base+':refs/heads/main')
class PreflightRemote(Remote):
 def __init__(self,b,head):super().__init__(b,head);self.refs={};self.preflight_pushes=[];self.preflight_fail=None
 def preflight_head(self,b,branch):return self.refs.get(branch)
 def push_preflight(self,b,repo,commit,branch):
  self.preflight_pushes.append((branch,commit))
  if self.preflight_fail=='before':raise RuntimeError('unobserved preflight failure')
  self.refs[branch]=commit;self.checks=[{'name':'controller','status':'in_progress','conclusion':None}];self.main_checks=[]
  if self.preflight_fail=='after':raise RuntimeError('lost preflight response')

class PreflightTests(unittest.TestCase):
 def setUp(self):
  Tests.setUp(self);self.b.update(preflight_branch_prefix='factory/verified',required_checks=['controller']);self.remote=PreflightRemote(self.b,self.base);self.p.remote=self.remote
 tearDown=Tests.tearDown
 git=Tests.git
 publish=Tests.publish
 def ready(self):self.remote.checks=[{'name':'controller','status':'completed','conclusion':'success'}]
 def test_factory_preflight_waits_for_exact_ci_then_publishes_and_replays_once(self):
  self.assertEqual(self.publish()['state'],'awaiting_ci');self.assertEqual(self.remote.pushes,[]);self.publish();self.assertEqual(len(self.remote.preflight_pushes),1)
  self.ready();self.remote.main_checks=[{'name':'controller','status':'completed','conclusion':'success'}];self.assertEqual(self.publish()['state'],'confirmed');self.publish();self.assertEqual(self.remote.pushes,[self.commit]);self.assertEqual(len(self.remote.preflight_pushes),1)
 def test_preflight_lost_response_reconciles_without_second_push(self):
  self.remote.preflight_fail='after'
  with self.assertRaises(RuntimeError):self.publish()
  self.remote.preflight_fail=None;self.assertEqual(self.publish()['state'],'awaiting_ci');self.ready();self.publish();self.assertEqual(len(self.remote.preflight_pushes),1)
 def test_unobserved_preflight_is_never_blindly_retried(self):
  self.remote.preflight_fail='before'
  with self.assertRaises(RuntimeError):self.publish()
  self.remote.preflight_fail=None
  with self.assertRaisesRegex(ValueError,'no blind retry'):self.publish()
  self.assertEqual(len(self.remote.preflight_pushes),1);self.assertEqual(self.remote.pushes,[])
 def test_failed_or_skipped_required_ci_never_reaches_main(self):
  self.publish()
  for verdict in ['failure','skipped','neutral']:
   self.remote.checks=[{'name':'controller','status':'completed','conclusion':verdict}]
   with self.assertRaisesRegex(ValueError,'CI failed'):self.publish()
  self.assertEqual(self.remote.pushes,[])
 def test_preflight_namespace_and_content_are_checked_before_upload(self):
  for overrides in [{'preflight_branch_prefix':'main'},{'allowed_paths':['docs']},{'required_checks':[]}]:
   with self.assertRaises(ValueError):self.p.publish({**self.b,**overrides},{'id':'goal'})
  self.assertEqual(self.remote.preflight_pushes,[])
 def test_whole_goal_completion_is_required_before_preflight_when_bound(self):
  self.b['require_goal_completion']=True
  with self.assertRaises(FileNotFoundError):self.publish()
  self.assertEqual(self.remote.preflight_pushes,[])
 def test_strict_main_ci_delayed_confirms_once_without_repush(self):
  self.assertEqual(self.publish()['state'],'awaiting_ci')
  self.ready()
  self.remote.main_checks=[]
  self.assertEqual(self.publish()['state'],'awaiting_ci')
  self.assertEqual(self.remote.pushes,[self.commit])
  self.remote.main_checks=[{'name':'controller','status':'completed','conclusion':'success'}]
  self.assertEqual(self.publish()['state'],'confirmed')
  self.publish()
  self.assertEqual(len(self.remote.pushes),1)
  self.assertEqual(len(M.read(self.p.root/'state.json')['history']),1)
 def test_strict_failed_main_ci_fences_without_second_push(self):
  self.assertEqual(self.publish()['state'],'awaiting_ci')
  self.ready()
  self.remote.main_checks=[]
  self.assertEqual(self.publish()['state'],'awaiting_ci')
  self.remote.main_checks=[{'name':'controller','status':'completed','conclusion':'failure'}]
  with self.assertRaisesRegex(ValueError,'CI failed'):self.publish()
  self.assertEqual(len(self.remote.pushes),1)
  self.assertEqual(M.read(self.p.root/'state.json')['history'],[])

if __name__=='__main__':unittest.main()
