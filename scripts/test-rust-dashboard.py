"""Differential dashboard checks: native CLI versus frozen Python loader/domain/render.
Synthetic fixtures only; no private capture or provider is accessed.
"""
import copy,html.parser,json,os,pathlib,subprocess,tempfile,unittest
from dashboard import loader,domain,render
ROOT=pathlib.Path(__file__).resolve().parents[1]
BIN=pathlib.Path(os.environ.get('FACTORY_FIXTURE_CONTROLLER',ROOT/'target/debug/code-factory-controller'))
ASOF='2026-01-01T13:00:00Z'
class Text(html.parser.HTMLParser):
 def __init__(self):super().__init__();self.values=[];self.ignored=0
 def handle_starttag(self,tag,attrs):
  if tag in ('style','script'):self.ignored+=1
 def handle_endtag(self,tag):
  if tag in ('style','script'):self.ignored-=1
 def handle_data(self,data):
  if not self.ignored and data.strip():self.values.append(' '.join(data.split()))
def visible(value):p=Text();p.feed(value);return p.values
class Parity(unittest.TestCase):
 def native(self,root,html=False):
  out=subprocess.run([str(BIN),'dashboard','--state-root',str(root),'--as-of',ASOF,'--print-html' if html else '--print-json'],capture_output=True,text=True,timeout=20)
  self.assertEqual(out.returncode,0,out.stderr)
  return out.stdout if html else json.loads(out.stdout)
 def test_existing_synthetic_trees(self):
  for name in ['mixed','no-metrics']:
   with self.subTest(name=name):
    root=ROOT/'dashboard/tests/fixtures'/name
    expected=domain.build_dashboard(loader.load_state(root),as_of=ASOF)
    self.assertEqual(self.native(root),expected)
    self.assertEqual(visible(self.native(root,html=True)),visible(render.render_html(expected)))
 def fixture(self,root):
  (root/'research-goals').mkdir();(root/'one/cycles/000001').mkdir(parents=True)
  def put(p,v):(root/p).write_text(json.dumps(v))
  put('research-goals/launch.json',{'worker_slots':1,'goals':[{'id':'one','root':'one','acceptance':{'required_evidence':['measured']}}]})
  put('research-goals/dispatcher-state.json',{'updated_utc':'2026-01-01T12:30:00Z','current':{'goal':'one','phase':'running','expected_sequence':1,'process':{'pid':1}},'goals':{'one':{'state':'ready','model':'fixture/free'}}})
  put('one/goal.json',{'objective':'Trusted <UI> /home/fixture/private-path','completion_criteria':['one']})
  put('one/status.json',{'status':'running','sequence':1,'current':{'sequence':1},'updated_utc':'2026-01-01T12:31:00Z','continuation':'Continue /tmp/hidden/path'})
  put('one/cycles/000001/result.json',{'status':'running','sequence':1,'started_utc':'2026-01-01T12:00:00Z','task':'PROJECT INSTRUCTIONS:\nPreserve <script> & paths /workspace/hidden'})
  return put
 def test_activity_time_accounting_mutations(self):
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d);put=self.fixture(root)
   for result in [
    {'status':'running','sequence':1,'started_utc':'2026-01-01T12:00:00Z'},
    {'status':'failed','sequence':1,'started_utc':'2026-01-01T12:00:00Z','ended_utc':'2026-01-01T12:01:00Z','usage':{'tokens':{'input':1,'output':2,'reasoning':3,'cache':{'read':4,'write':5}},'cost':0,'accounting_status':'unknown'}},
    {'status':'accepted','sequence':1,'started_utc':'invalid','ended_utc':'2026-01-01T12:01:00Z','usage':{'tokens':{'input':1111,'output':22,'reasoning':33,'cache':{'read':44,'write':55}},'cost':0,'accounting_status':'settled_zero'}},
   ]:
    put('one/cycles/000001/result.json',result)
    expected=domain.build_dashboard(loader.load_state(root),as_of=ASOF)
    self.assertEqual(self.native(root),expected)
    self.assertEqual(visible(self.native(root,html=True)),visible(render.render_html(expected)))
 def test_capture_denied_escape_and_hash_bindings(self):
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d);put=self.fixture(root)
   put('capture.json',{'captured_utc':ASOF,'records':[{'path':'one/goal.json','captured_sha256':loader.sha256_file(root,'one/goal.json')},{'path':'one/status.json','captured_sha256':'bad'},{'path':'../outside','captured_sha256':'bad'},{'path':'one/private.json','captured_sha256':'bad'}]})
   self.assertEqual(self.native(root),domain.build_dashboard(loader.load_state(root),as_of=ASOF))
 def test_loader_refusals_are_errors_and_do_not_expose_absolute_paths(self):
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d);put=self.fixture(root)
   put('research-goals/launch.json',{'goals':[{'id':'one','root':'../private'}]})
   out=subprocess.run([str(BIN),'dashboard','--state-root',str(root),'--print-json'],capture_output=True,text=True)
   self.assertNotEqual(out.returncode,0)
   self.assertNotIn(str(root),out.stderr)
 def test_native_http_is_loopback_get_only_and_refreshes_without_writes(self):
  import select,urllib.request,urllib.error
  with tempfile.TemporaryDirectory() as d:
   root=pathlib.Path(d);put=self.fixture(root);before={p.relative_to(root):p.read_bytes() for p in root.rglob('*') if p.is_file()}
   server=subprocess.Popen([str(BIN),'dashboard','--state-root',str(root),'--as-of',ASOF,'--port','0'],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
   try:
    self.assertTrue(select.select([server.stdout],[],[],10)[0],'native HTTP startup deadline')
    line=server.stdout.readline();self.assertIn('http://127.0.0.1:',line);url=line.split('http://',1)[1].split('/',1)[0]
    def get():
     with urllib.request.urlopen('http://'+url+'/api/state',timeout=5) as response:
      self.assertEqual(response.headers['Cache-Control'],'no-store');return json.load(response)
    first=get();self.assertEqual(first['projects'][0]['goal_id'],'one')
    with self.assertRaises(urllib.error.HTTPError) as error:urllib.request.urlopen(urllib.request.Request('http://'+url+'/api/state',data=b'{}',method='POST'),timeout=5)
    self.assertEqual(error.exception.code,405)
    after={p.relative_to(root):p.read_bytes() for p in root.rglob('*') if p.is_file()};self.assertEqual(after,before)
    put('one/goal.json',{'objective':'Updated state from the owner'});self.assertEqual(get()['projects'][0]['objective'],'Updated state from the owner')
    (root/'research-goals/launch.json').unlink()
    with self.assertRaises(urllib.error.HTTPError) as error:urllib.request.urlopen('http://'+url+'/',timeout=5)
    self.assertEqual(error.exception.code,500);self.assertNotIn(str(root),error.exception.read().decode())
   finally:server.terminate();server.communicate(timeout=5)
  bad=subprocess.run([str(BIN),'dashboard','--host','0.0.0.0'],capture_output=True,text=True,timeout=5);self.assertNotEqual(bad.returncode,0);self.assertIn('loopback',bad.stderr)
if __name__=='__main__':unittest.main(verbosity=2)
