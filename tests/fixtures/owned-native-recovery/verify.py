"""Direct-argv verifier for this frozen input bundle; no output-filter pipeline.

The owner must pin/copy this entire bundle into the existing read-only data mount.
This file does not change a live goal, mount, policy or publication configuration.
"""
import hashlib,json,os,pathlib,re,subprocess,sys

def main():
 if len(sys.argv)!=1:raise ValueError('Verifier accepts no shell command or path override')
 root=pathlib.Path(__file__).resolve().parent
 manifest=json.loads((root/'manifest.json').read_text())
 check=root/'check.py'
 if check.is_symlink() or hashlib.sha256(check.read_bytes()).hexdigest()!=manifest['check_sha256']:
  raise ValueError('Frozen regression bytes changed')
 commit=os.environ.get('FACTORY_VERIFICATION_COMMIT','')
 if not re.fullmatch('[0-9a-f]{40}',commit):raise ValueError('Exact verification commit required')
 p=subprocess.run([sys.executable,'-I',str(check)],capture_output=True,text=True,timeout=60)
 # Full child diagnostics are retained. Filtering/presentation cannot replace
 # the regression exit status, and zero exit alone never attests execution.
 sys.stderr.write(p.stdout+p.stderr)
 if p.returncode:return p.returncode if p.returncode>0 else 1
 ran=re.findall(r'^Ran (\d+) tests? in ',p.stderr,re.M)
 if ran!=[str(manifest['expected_tests'])] or not re.search(r'^OK\s*$',p.stderr,re.M) or re.search(r'^(?:FAILED|ERROR|Traceback)|\bskipped=',p.stderr,re.M):
  raise ValueError('Regression lacks a positive complete test result')
 print(json.dumps({'passed':True,'skipped':False,'failed':0,'executed':int(ran[0]),'commit':commit,'evidence_type':'runtime_tests','kind':'Injected native effects; actual dispatcher regression','check_sha256':manifest['check_sha256']}))
 return 0

if __name__=='__main__':
 try:sys.exit(main())
 except Exception as error:print(str(error),file=sys.stderr);sys.exit(1)
