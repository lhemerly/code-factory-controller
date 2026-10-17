// Independent synthetic kernel probe; never touches the denied real repository.
import {mkdtemp,mkdir,writeFile,readFile,rm} from 'node:fs/promises';
import {resolve} from 'node:path';import {pathToFileURL} from 'node:url';import {execFileSync} from 'node:child_process';import assert from 'node:assert/strict';
const source=resolve(import.meta.dirname,'spark-sandbox.mjs');const {sandbox}=await import(pathToFileURL(source));
const root=await mkdtemp('/tmp/factory-namespace-review-'),repo=resolve(root,'owner'),work=resolve(root,'worker'),plugin=resolve(root,'plugin'),profile=resolve(root,'profile');
try{
for(const p of [repo,plugin,profile])await mkdir(p);
const git=(...args)=>execFileSync('/usr/bin/git',['-C',repo,...args],{encoding:'utf8'}).trim();
git('init','-q');await writeFile(resolve(repo,'frozen.txt'),'frozen');await writeFile(resolve(repo,'source.py'),'print("BASELINE")\n');git('add','.');git('-c','user.name=Fixture','-c','user.email=fixture@invalid','commit','-qm','base');git('worktree','add','--detach',work,'HEAD');
await writeFile(resolve(repo,'owner-only.py'),'print("OWNER_ONLY_REACHED")\n');await writeFile(resolve(work,'opencode.json'),'{}');await mkdir(resolve(work,'.opencode/pstack-admission'),{recursive:true});
const cfg={plugin,native:'/usr/bin/true'};const args=sandbox(cfg,repo,work,profile,['frozen.txt']);args.pop();
const probe=`import pathlib,subprocess,json,errno,os
w=pathlib.Path(${JSON.stringify(work)});r=pathlib.Path(${JSON.stringify(repo)});v={}
v['assigned_cwd']=pathlib.Path.cwd()==w
(w/'allowed.txt').write_text('allowed');v['worker_writable']=(w/'allowed.txt').read_text()=='allowed'
v['owner_source_hidden']=not (r/'source.py').exists()
v['owner_canary_hidden']=not (r/'owner-only.py').exists()
q=subprocess.run(['/usr/bin/python3',str(r/'owner-only.py')],capture_output=True,text=True);v['owner_absolute_execution_blocked']=q.returncode!=0 and 'OWNER_ONLY_REACHED' not in q.stdout
(w/'alias.py').symlink_to(r/'owner-only.py');v['owner_symlink_hidden']=not (w/'alias.py').exists()
for n in ['.git','frozen.txt']:
 try:(w/n).write_text('bad');v[n+'_readonly']=False
 except OSError as e:v[n+'_readonly']=e.errno==errno.EROFS
v['git_reads_work']=subprocess.check_output(['/usr/bin/git','show','HEAD:source.py'],text=True)=='print("BASELINE")\\n'
v['git_status_work']=subprocess.run(['/usr/bin/git','status','--porcelain'],capture_output=True).returncode==0
try:(r/'.git'/'attempted-write').write_text('bad');v['git_metadata_readonly']=False
except OSError as e:v['git_metadata_readonly']=e.errno==errno.EROFS
print(json.dumps(v))`;
const checks=JSON.parse(execFileSync('/usr/bin/bwrap',[...args,'/usr/bin/python3','-c',probe],{encoding:'utf8'}));
for(const [name,value] of Object.entries(checks))assert.equal(value,true,name);
console.log(JSON.stringify({passed:true,executed_checks:Object.keys(checks).length,checks,evidence:'Synthetic real Git worker namespace; no native or model calls'}));
}finally{await rm(root,{recursive:true,force:true})}
