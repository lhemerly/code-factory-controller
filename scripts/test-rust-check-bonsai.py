"""Host synthetic transport parity: no OpenCode or model inference."""
import os
import pathlib,sys,importlib.util,json,copy,subprocess,threading,unittest,contextlib
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
BASE=pathlib.Path(__file__).resolve().parents[1];native=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',BASE/'target/debug/code-factory-controller')).resolve();sp=importlib.util.spec_from_file_location('frozen_bonsai',BASE/'scripts/check-bonsai.py');legacy=importlib.util.module_from_spec(sp);sp.loader.exec_module(legacy);profile=json.loads((BASE/'examples/opencode.bonsai.json').read_text())
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  self.server.paths.append(self.path);mode=self.server.mode
  if self.path=='/global/health':value={'healthy':True,'version':'1.19.0'}
  elif self.path=='/config':value=self.server.config
  elif self.path=='/provider':
   model=copy.deepcopy(self.server.config['provider'][legacy.PROVIDER_ID]['models'][legacy.MODEL_ID]);model['cost']={'input':0,'output':0,'cache':{'read':0,'write':0}};value={'connected':[legacy.PROVIDER_ID],'all':[{'id':legacy.PROVIDER_ID,'models':{legacy.MODEL_ID:model}}]}
  elif self.path=='/v1/models':value={'data':[{'id':legacy.MODEL_ID}]}
  else:self.send_error(404);return
  status=503 if mode=='non2xx' else (302 if mode=='redirect' else 200);self.send_response(status);self.send_header('Content-Type','application/json')
  if mode=='redirect':self.send_header('Location',self.server.url+'/redirect-target')
  self.end_headers()
  try:
   if mode=='oversized':
    self.wfile.write(b'{"pad":"');chunk=b'x'*65536
    for _ in range(80):self.wfile.write(chunk);self.wfile.flush()
    self.wfile.write(b'"}')
   else:self.wfile.write(json.dumps(value).encode())
  except (BrokenPipeError,ConnectionResetError):pass
@contextlib.contextmanager
def fixture(mode):
 server=ThreadingHTTPServer(('127.0.0.1',0),Handler);server.mode=mode;server.paths=[];server.url='http://127.0.0.1:'+str(server.server_port);server.config=copy.deepcopy(profile);server.config['provider'][legacy.PROVIDER_ID]['options']['baseURL']=server.url+'/v1';thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
 try:yield server
 finally:server.shutdown();server.server_close();thread.join(timeout=3)
class NativeBonsai(unittest.TestCase):
 def invoke(self,server):return subprocess.run([str(native),'check-bonsai','--opencode-url',server.url],capture_output=True,text=True,timeout=30)
 def test_valid_fixture_matches_frozen_python_reference(self):
  with fixture('valid') as s:
   self.assertTrue(legacy.check(s.url)['passed']);result=self.invoke(s);self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(json.loads(result.stdout)['passed'])
 def test_non2xx_valid_json_is_rejected(self):
  with fixture('non2xx') as s:
   with self.assertRaises(Exception):legacy.check(s.url)
   result=self.invoke(s);self.assertNotEqual(result.returncode,0);self.assertEqual(s.paths,['/global/health','/global/health'])
 def test_redirect_is_not_followed(self):
  with fixture('redirect') as s:
   result=self.invoke(s);self.assertNotEqual(result.returncode,0);self.assertEqual(s.paths,['/global/health'])
 def test_oversized_response_is_rejected(self):
  with fixture('oversized') as s:
   result=self.invoke(s);self.assertNotEqual(result.returncode,0);self.assertIn('limit',json.loads(result.stdout).get('error','').lower());self.assertEqual(s.paths,['/global/health'])
suite=unittest.defaultTestLoader.loadTestsFromTestCase(NativeBonsai);r=unittest.TextTestRunner(verbosity=2).run(suite);raise SystemExit(not r.wasSuccessful())
