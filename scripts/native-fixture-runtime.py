"""Test-only immutable compact executable copy; never changes the built artifact.
Debug sections make repeated SHA-256 fixture audits needlessly expensive in debug
Rust. The copied executable has identical code and is independently pinned by all
fixture manifests; keep it alive until the returned TemporaryDirectory closes.
"""
import os,pathlib,subprocess,tempfile

def native_binary(root):
 source=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',root/'target/debug/code-factory-controller')).resolve()
 temp=tempfile.TemporaryDirectory(prefix='factory-native-fixture-')
 binary=pathlib.Path(temp.name)/'code-factory-controller'
 subprocess.run(['strip','--strip-debug','-o',str(binary),str(source)],check=True)
 binary.chmod(0o700)
 return temp,binary
