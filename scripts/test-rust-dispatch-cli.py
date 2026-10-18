"""Execute frozen real-controller dispatch fixtures through the native owner CLI.
Python supplies test fixtures only; model transport is fake.
"""
import pathlib, subprocess
S=pathlib.Path(__file__).resolve().parent
import importlib.util,os
spec=importlib.util.spec_from_file_location('native_fixture_runtime',S/'native-fixture-runtime.py');helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
fixture_temp,fixture_binary=helper.native_binary(S.parent)
os.environ['FACTORY_FIXTURE_CONTROLLER']=str(fixture_binary)
source=(S/'test-dispatch-cli.py').read_text()
source=source.replace("argv=['/usr/bin/python3',str(dispatcher),'run'", "argv=[str(binary),'research-dispatch','run'")
source=source.replace("binary=root.parent/'target/debug/code-factory-controller'", "binary=pathlib.Path(os.environ['FACTORY_FIXTURE_CONTROLLER'])")
exec(compile(source,str(S/'test-dispatch-cli.py'),'exec'),{'__file__':str(S/'test-dispatch-cli.py'),'__name__':'__main__'})
