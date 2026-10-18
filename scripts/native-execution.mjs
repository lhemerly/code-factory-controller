// Own the native server with the existing Linux worker-profile supervisor.
// Closing this handle's control pipe is the only stop request; the kernel owns
// descendants and supplies the independent quiescence receipt.
import {spawn} from 'node:child_process';
import {mkdir,readFile,open} from 'node:fs/promises';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {createHash,randomBytes} from 'node:crypto';
const digest=x=>createHash('sha256').update(x).digest('hex');
const delay=ms=>new Promise(ok=>setTimeout(ok,ms));
const identity=r=>r?.version===1&&r.observer==='linux_subreaper_waitpid_pidfd';
const environmentKeys=['PATH','HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','CARGO_HOME','CARGO_TARGET_DIR','RUSTUP_HOME','RUSTUP_TOOLCHAIN','RUSTFLAGS','CC','CXX','PKG_CONFIG_PATH','LD_LIBRARY_PATH','VIRTUAL_ENV','PYTHONPATH','PYTHONNOUSERSITE','GIT_AUTHOR_NAME','GIT_AUTHOR_EMAIL','GIT_COMMITTER_NAME','GIT_COMMITTER_EMAIL','PYTHONDONTWRITEBYTECODE','OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MPLCONFIGDIR','PSTACK_FACTORY_URL','PSTACK_FACTORY_PROJECT','PSTACK_FACTORY_TOKEN'];
export function nativeEnvironment(env){return Object.fromEntries(environmentKeys.filter(k=>typeof env[k]==='string').map(k=>[k,env[k]]))}
export async function startNativeExecution({controller,argv,env,cwd,directory,commit,ready,shouldStop,onProcess,red=s=>s,sleep=delay}){
 const id=randomBytes(16).toString('hex'),dir=resolve(directory,'native-execution-'+id);await mkdir(dir,{mode:0o700});
 const planPath=resolve(dir,'plan.json'),receiptPath=resolve(dir,'process.json'),startedPath=resolve(dir,'process.started.json');
 const vars=nativeEnvironment(env);
 const plan={execution_id:id,argv:['/usr/bin/env',...Object.entries(vars).map(([k,v])=>k+'='+v),...argv],cwd,log:resolve(dir,'stderr.log'),stdout:resolve(dir,'stdout.log'),commit,seconds:0,receipt:receiptPath};
 const bytes=JSON.stringify(plan)+'\n',f=await open(planPath,'wx',0o600);try{await f.writeFile(bytes);await f.sync()}finally{await f.close()}
 const boot=(await readFile('/proc/sys/kernel/random/boot_id','utf8')).trim();
 const p=spawn(controller,['agy-supervisor','--plan',planPath],{cwd,env:{PATH:vars.PATH??'/usr/bin:/bin'},stdio:['pipe','ignore','pipe']});
 let diagnostic='';p.stderr.on('data',b=>diagnostic=(diagnostic+b.toString()).slice(-65536));
 const log=()=>[plan.stdout,plan.log].map(path=>{try{return readFileSync(path,'utf8')}catch(e){if(e.code==='ENOENT')return '';throw e}}).join('\n');
 p._log=()=>red(log()+'\n'+diagnostic);
 p._ownedNative={id,boot,planPath,receiptPath,startedPath,plan_sha256:digest(bytes),cwd,commit};
 // Retain the actual handle before waiting for readiness, including startup failure.
 onProcess(p);
 for(let i=0;i<300;i++){
  const found=ready(log());
  if(found){
   const started=JSON.parse(await readFile(startedPath,'utf8'));
   if(!identity(started)||started.execution_id!==id||started.supervisor.pid!==p.pid||started.supervisor.boot_id!==boot||!Number.isInteger(started.root_pid))throw Error('Native execution ownership observation unknown');
   p._ownedNative.started=started;return {p,...found};
  }
  if(p.exitCode!==null||p.signalCode!==null)throw Error('Owned native startup failed: '+p._log());
  if(shouldStop())throw Error('Operator stopping');
  await sleep(100);
 }
 throw Error('Owned native startup timeout');
}
export async function stopNativeExecution(p,{timeoutMs=10000}={}){
 const h=p._ownedNative;if(!h)throw Error('Native owned execution handle required');
 if(p.exitCode===null&&p.signalCode===null){
  const exited=new Promise(ok=>p.once('exit',ok));
  if(!p.stdin.destroyed&&!p.stdin.writableEnded)p.stdin.end();
  let timer;try{await Promise.race([exited,new Promise((_,bad)=>{timer=setTimeout(()=>bad(Error('Native owned stop receipt pending')),timeoutMs)})])}finally{clearTimeout(timer)}
 }
 const receipt=JSON.parse(await readFile(h.receiptPath,'utf8'));
 if(!identity(receipt)||receipt.execution_id!==h.id||receipt.supervisor.pid!==p.pid||receipt.supervisor.boot_id!==h.boot||receipt.quiescent!==true||receipt.leaked_descendants!==false||receipt.cleanup_error!==null||receipt.output_truncated!==false||!Number.isInteger(receipt.root_exit_status))throw Error('Native owned quiescence unconfirmed; preserve its execution records');
 if(h.started&&(receipt.supervisor.start_ticks!==h.started.supervisor.start_ticks||receipt.root_pid!==h.started.root_pid))throw Error('Native execution stop identity changed');
 h.receipt=receipt;
 return {execution_id:h.id,plan_sha256:h.plan_sha256,process_receipt:h.receiptPath,worktree:h.cwd,commit:h.commit,receipt};
}
export function nativeExecutionBinding(p){const h=p._ownedNative;if(!h?.started)throw Error('Native owned start required');return {execution_id:h.id,plan_sha256:h.plan_sha256,started:h.started}}

// A new source link could point into the intentionally writable native profile
// or admission metadata. Compare against the approved scope base, including WIP
// carried across attempts; the immutable source proof cannot accept that alias.
export function assertImmutableSourceCandidate(raw){
 if(/^:\d{6} 120000 /m.test(raw))throw Error('Changed source symlink cannot satisfy immutable source barrier');
}
