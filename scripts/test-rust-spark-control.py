"""Host-owned parity: frozen pre-migration control assertions against Rust CLI."""
import os
import importlib.util,pathlib,subprocess,sys,unittest,json
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve()
spec=importlib.util.spec_from_file_location('frozen_control',BASE/'scripts/test-spark-control.py');legacy=importlib.util.module_from_spec(spec);spec.loader.exec_module(legacy)
class RustControls(legacy.Controls):
 def run_control(self,*args):
  return subprocess.run([str(native),'spark-control',*args,'--root',str(self.root)],capture_output=True,text=True,timeout=15)
suite=unittest.TestSuite(unittest.defaultTestLoader.loadTestsFromTestCase(RustControls));result=unittest.TextTestRunner(verbosity=2).run(suite)
raise SystemExit(not result.wasSuccessful())
