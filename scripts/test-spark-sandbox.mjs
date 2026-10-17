// Actual kernel mount/PID sandbox controls. No model/provider calls.
import {mkdtemp,mkdir,writeFile,readFile,rm} from 'node:fs/promises';import {resolve} from 'node:path';import {tmpdir} from 'node:os';import {execFileSync} from 'node:child_process';import assert from 'node:assert/strict';import {sandbox} from './spark-sandbox.mjs';
const root=await mkdtemp(resolve(tmpdir(),'spark-sandbox-')),repo=resolve(root,'repository'),work=resolve(root,'work'),profile=resolve(root,'native');
try{
 for(const p of [repo,resolve(repo,'.git'),work,profile,resolve(work,'.opencode/pstack-admission'),resolve(root,'data'),resolve(root,'venv')])await mkdir(p,{recursive:true});
 for(const p of ['.git','opencode.json','frozen.py'])await writeFile(resolve(work,p),'frozen');
 await writeFile(resolve(root,'operator-secret'),'outside-boundary');await writeFile(resolve(root,'data/ct'),'original');
 const cfg={plugin:resolve(import.meta.dirname,'../../opencode-pstack'),native:resolve(import.meta.dirname,'../../runtime/node_modules/@opencode/cli-linux-x64/bin/opencode'),data:resolve(root,'data'),venv:resolve(root,'venv')};
 const code=`import os,pathlib,json,errno
work=pathlib.Path(${JSON.stringify(work)});results={}
for name in ['.git','opencode.json','frozen.py']:
 try:(work/name).write_text('forbidden');results[name]=False
 except OSError as e:results[name]=e.errno==errno.EROFS
try:(work/'.opencode/plugins').mkdir();results['plugin_injection_blocked']=False
except OSError as e:results['plugin_injection_blocked']=e.errno==errno.EROFS
(work/'implementation.py').write_text('allowed');results['code_write_allowed']=True
(work/'.opencode/pstack-admission/owned').mkdir();results['pstack_lock_allowed']=True
results['operator_secret_hidden']=not pathlib.Path(${JSON.stringify(resolve(root,'operator-secret'))}).exists()
results['resolver_readable']=pathlib.Path('/etc/resolv.conf').is_file()
results['actual_cwd']=str(pathlib.Path.cwd())==str(work)
print(json.dumps(results))
`;
 const args=sandbox(cfg,repo,work,profile,['frozen.py']);args.pop();const result=JSON.parse(execFileSync('/usr/bin/bwrap',[...args,'/usr/bin/python3','-c',code],{encoding:'utf8'}));for(const [k,v] of Object.entries(result))assert.equal(v,true,k);console.log(JSON.stringify({executed_checks:Object.keys(result).length,passed:true,checks:result},null,2));
}finally{await rm(root,{recursive:true,force:true})}
