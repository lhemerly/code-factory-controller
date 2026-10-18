"""Real native baseline/receipt restart with a PATH containing no Python.
The Python harness prepares the fixture only. No worker/provider is invoked.
"""
import hashlib,json,os,pathlib,shutil,subprocess,tempfile
S=pathlib.Path(__file__).resolve().parent;binary=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',S.parent/'target/debug/code-factory-controller')).resolve()
with tempfile.TemporaryDirectory(prefix='factory-native-path-') as d:
 root=pathlib.Path(d);goalroot=root/'goal';repo=goalroot/'repository';repo.mkdir(parents=True);(repo/'product.txt').write_text('native result\n')
 for args in [['init','-q'],['add','.'],['-c','core.hooksPath=/dev/null','-c','user.name=Fixture','-c','user.email=fixture@invalid','commit','-qm','fixture']]:subprocess.run(['/usr/bin/git','-C',str(repo),*args],check=True)
 commit=subprocess.check_output(['/usr/bin/git','-C',str(repo),'rev-parse','HEAD'],text=True).strip();pathdir=root/'path';pathdir.mkdir()
 for name in ['git','cat']:(pathdir/name).symlink_to('/usr/bin/'+name)
 assert shutil.which('python',path=str(pathdir)) is None and shutil.which('python3',path=str(pathdir)) is None
 evidence='test "$(cat product.txt)" = "native result" && printf \'{"passed":true,"skipped":false,"executed":1,"failed":0,"commit":"%s","evidence_type":"native_baseline"}\' "$FACTORY_VERIFICATION_COMMIT"'
 goal={'id':'native-baseline','root':str(goalroot),'repository':str(repo),'initial_base':commit,'cycle_command':['/bin/false'],'acceptance':{'required_evidence':['native_baseline'],'completion_checks':[{'id':'native-check','evidence_type':'native_baseline','argv':['/bin/sh','-c',evidence]}]}}
 manifest={'goals':[goal],'worker_slots':1,'controller':str(binary),'pool_lock':str(root/'pool.lock'),'state':str(root/'state.json'),'trusted_files':{str(binary):hashlib.sha256(binary.read_bytes()).hexdigest()}}
 mp=root/'manifest.json';mp.write_text(json.dumps(manifest));argv=[str(binary),'research-dispatch','run','--manifest',str(mp)]
 for _ in range(2):
  out=subprocess.run(argv,env={'PATH':str(pathdir)},capture_output=True,text=True,timeout=30);assert out.returncode==0,(out.stdout,out.stderr)
 state=json.loads((root/'state.json').read_text());gs=state['goals']['native-baseline'];assert state['status']=='complete' and gs['state']=='complete';assert not gs['milestones'];assert gs['goal_acceptance']['receipts'][0]['executor']['quiescent'];assert not (goalroot/'status.json').exists()
 print(json.dumps({'passed':True,'python_on_runtime_path':False,'real_native_whole_goal_baseline':True,'restart_audited':True,'worker_dispatched':False,'inference':False}))
