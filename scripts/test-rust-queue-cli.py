"""Frozen queue + real-controller fixture through Rust intake and owner commands."""
import pathlib
S=pathlib.Path(__file__).resolve().parent
import importlib.util,os
spec=importlib.util.spec_from_file_location('native_fixture_runtime',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
fixture_temp,fixture_binary=helper.native_binary(S.parent)
os.environ['FACTORY_FIXTURE_CONTROLLER']=str(fixture_binary)
source=(S/'test-queue-cli.py').read_text().replace("cli=[sys.executable,str(S/'research-dispatch.py')]", "cli=[str(binary),'research-dispatch']")
source=source.replace("'Another owner holds'", "'another owner holds'").replace('time.monotonic()+15', 'time.monotonic()+90')
source=source.replace("binary=S.parent/'target/debug/code-factory-controller'", "binary=pathlib.Path(os.environ['FACTORY_FIXTURE_CONTROLLER'])")
exec(compile(source,str(S/'test-queue-cli.py'),'exec'),{'__file__':str(S/'test-queue-cli.py'),'__name__':'__main__'})
