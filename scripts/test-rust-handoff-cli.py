"""Frozen cooperative worker-boundary handoff through native owner + helper CLIs."""
import importlib.util,os,pathlib,unittest
S=pathlib.Path(__file__).resolve().parent
sp=importlib.util.spec_from_file_location('native_fixture',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(sp);sp.loader.exec_module(helper);fixture_temp,binary=helper.native_binary(S.parent);os.environ['FACTORY_FIXTURE_CONTROLLER']=str(binary)
source=(S/'test-queue-handoff.py').read_text()
source=source.replace("owner=subprocess.Popen([sys.executable,str(ROOT/'research-dispatch.py'),'run'", "owner=subprocess.Popen([os.environ['FACTORY_FIXTURE_CONTROLLER'],'research-dispatch','run'")
source=source.replace("helper=subprocess.Popen([sys.executable,str(ROOT/'queue-handoff.py'),'--manifest'", "helper=subprocess.Popen([os.environ['FACTORY_FIXTURE_CONTROLLER'],'queue-handoff','--manifest'")
source=source.replace("'trusted_files':{str(worker):H.D.queue_api.sha(worker)}", "'trusted_files':{str(worker):H.D.queue_api.sha(worker),os.environ['FACTORY_FIXTURE_CONTROLLER']:H.D.queue_api.sha(os.environ['FACTORY_FIXTURE_CONTROLLER'])}")
old="replacement.write_text('import pathlib;pathlib.Path('+repr(str(marker))+').write_text(\"replacement\")')"
new="replacement.write_text('#!/bin/sh\\nprintf replacement > '+str(marker)+'\\n');replacement.chmod(0o700)"
assert old in source;source=source.replace(old,new)
source=source.replace('time.monotonic()+5','time.monotonic()+30').replace('communicate(timeout=8)','communicate(timeout=30)').replace('communicate(timeout=2)','communicate(timeout=15)')
source=source.replace("self.fail('handoff fixture event timeout')", "self.fail('handoff fixture event timeout: '+(owner.stderr.read().decode() if owner.poll() is not None else 'owner still running; state='+state.read_text()+'; log='+((goalroot/'dispatcher-console.log').read_text() if (goalroot/'dispatcher-console.log').exists() else 'no log')))")
namespace={'__file__' :str(S/'test-queue-handoff.py'),'__name__':'native_handoff_fixture'};exec(compile(source,str(S/'test-queue-handoff.py'),'exec'),namespace)
suite=unittest.TestSuite([namespace['Tests']('test_cooperative_handoff_waits_for_child_completion_and_releases_only_own_pauses')]);result=unittest.TextTestRunner(verbosity=2).run(suite);raise SystemExit(not result.wasSuccessful())
