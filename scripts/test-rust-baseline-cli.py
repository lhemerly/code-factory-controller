"""Frozen baseline-evaluation CLI fixture through native Rust coordinator."""
import pathlib
S=pathlib.Path(__file__).resolve().parent
import importlib.util,os
spec=importlib.util.spec_from_file_location('native_fixture_runtime',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
fixture_temp,fixture_binary=helper.native_binary(S.parent)
os.environ['FACTORY_FIXTURE_CONTROLLER']=str(fixture_binary)
source=(S/'test-goal-baseline-cli.py').read_text().replace("argv=[sys.executable,str(ROOT/'research-dispatch.py'),'run'", "argv=[str(binary),'research-dispatch','run'")
source=source.replace("binary=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',ROOT.parent/'target/debug/code-factory-controller')).resolve()", "binary=pathlib.Path(os.environ['FACTORY_FIXTURE_CONTROLLER'])")
exec(compile(source,str(S/'test-goal-baseline-cli.py'),'exec'),{'__file__':str(S/'test-goal-baseline-cli.py'),'__name__':'__main__'})
