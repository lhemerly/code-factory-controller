// Coding-only kernel mount boundary; no OpenCode, provider or model invocation.
import {mkdtemp,mkdir,writeFile,rm} from 'node:fs/promises';
import {resolve} from 'node:path';import {tmpdir} from 'node:os';
import {execFileSync} from 'node:child_process';import assert from 'node:assert/strict';
import {sandbox,workerPermissions} from './spark-sandbox.mjs';
const root=await mkdtemp(resolve(tmpdir(),'coding-sandbox-'));
try{
 const repo=resolve(root,'repository'),work=resolve(root,'work'),profile=resolve(root,'native'),plugin=resolve(root,'plugin');
 for(const p of [repo,resolve(repo,'.git'),work,profile,plugin,resolve(work,'.opencode/pstack-admission')])await mkdir(p,{recursive:true});
 for(const name of ['.git','opencode.json','frozen.py'])await writeFile(resolve(work,name),'frozen');
 await writeFile(resolve(root,'operator-secret'),'outside-boundary');
 const cfg={plugin,native:'/usr/bin/true'};
 const data=resolve(root,'approved-data');await mkdir(data);await writeFile(resolve(data,'input'),'approved read-only input');
 const temporary=[{action:'external_directory',resource:'/tmp',effect:'allow'},{action:'external_directory',resource:'/tmp/**',effect:'allow'}];
 assert.deepEqual(workerPermissions(cfg,work).filter(r=>r.action==='external_directory'),temporary);
 assert.deepEqual(workerPermissions({...cfg,data},work).filter(r=>r.action==='external_directory'),[...temporary,{action:'external_directory',resource:data,effect:'allow'},{action:'external_directory',resource:data+'/**',effect:'allow'}]);
 assert.throws(()=>workerPermissions({...cfg,data:'relative'},work),/absolute/);
 const args=sandbox(cfg,repo,work,profile,['frozen.py']);
 assert(args.every(p=>typeof p==='string'));assert(!args.includes('undefined'));
 assert(!args.includes('--size'),'User approved existing tmpfs capacity without an added cap');
 for(const value of ['',42,'relative-data'])assert.throws(()=>sandbox({...cfg,data:value},repo,work,profile,[]),/absolute paths/);
 const research=sandbox({...cfg,venv:'/approved/venv',data:'/approved/data'},repo,work,profile,[]);
 assert(research.includes('/approved/data'));assert(research.includes('/approved/venv'));
 args.pop();
 const code=`import pathlib,errno,json
w=pathlib.Path(${JSON.stringify(work)});result={}
(w/'implementation.py').write_text('allowed');result['code_write_allowed']=True
for name in ['.git','opencode.json','frozen.py']:
 try:(w/name).write_text('forbidden');result[name]=False
 except OSError as error:result[name]=error.errno==errno.EROFS
result['operator_secret_hidden']=not pathlib.Path(${JSON.stringify(resolve(root,'operator-secret'))}).exists()
result['no_research_resources_required']=not (w/'data').exists() and not (w/'.venv').exists()
result['actual_code_cwd']=pathlib.Path.cwd()==w
print(json.dumps(result))
`;
 const observed=JSON.parse(execFileSync('/usr/bin/bwrap',[...args,'/usr/bin/python3','-c',code],{encoding:'utf8'}));
 for(const [name,value] of Object.entries(observed))assert.equal(value,true,name);
 const dataArgs=sandbox({...cfg,data},repo,work,profile,['frozen.py']);dataArgs.pop();
 const dataCode=`import pathlib,errno,json
p=pathlib.Path(${JSON.stringify(resolve(data,'input'))});result={'approved_data_readable':p.read_text()=='approved read-only input'}
try:p.write_text('forbidden');result['approved_data_stays_readonly']=False
except OSError as e:result['approved_data_stays_readonly']=e.errno==errno.EROFS
print(json.dumps(result))`;
 const dataObserved=JSON.parse(execFileSync('/usr/bin/bwrap',[...dataArgs,'/usr/bin/python3','-c',dataCode],{encoding:'utf8'}));
 for(const [name,value] of Object.entries(dataObserved))assert.equal(value,true,name);
 console.log(JSON.stringify({passed:true,evidence:'actual kernel sandbox, no native runtime or model',executed_checks:Object.keys(observed).length+Object.keys(dataObserved).length,checks:{...observed,...dataObserved}},null,2));
}finally{await rm(root,{recursive:true,force:true})}

await import('./test-worker-namespace.mjs');
