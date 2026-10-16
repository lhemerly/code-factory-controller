#!/usr/bin/env node
// Authorized host supervisor. No model execution scheduler or acceptance ledger in pstack.
import {spawn,execFileSync} from 'node:child_process';
import {mkdir,writeFile,readFile,rename,open,stat,symlink,access} from 'node:fs/promises';
import {resolve,dirname} from 'node:path';
import {createHash,randomBytes} from 'node:crypto';
import {createServer} from 'node:net';
import {OpenCode} from './opencode-v2-client.mjs';
import {buildResearchContext} from './research-context.mjs';
import {OwnershipLease,observeResearch,awaitNativeShells} from './continuous-ownership.mjs';
import {sandbox} from './spark-sandbox.mjs';
import {freeModels,classify,route,failure,recoveryAction,replacementAllowed,approvedScope} from './free-model-policy.mjs';
const oneCycle=process.argv.includes('--one-cycle');
const root=resolve(process.argv[2]),cfg=JSON.parse(await readFile(resolve(root,'supervisor.json')));
const goal=JSON.parse(await readFile(resolve(root,'goal.json'))),repo=resolve(root,'repository');
const protectedPaths=JSON.parse(await readFile(resolve(root,'protected.json')));
const delay=ms=>new Promise(r=>setTimeout(r,ms));const sha=b=>createHash('sha256').update(b).digest('hex');
const now=()=>new Date().toISOString();let shutdown=false;process.on('SIGTERM',()=>shutdown=true);process.on('SIGINT',()=>shutdown=true);
const env={PATH:cfg.path,HOME:resolve(root,'host-home'),GIT_AUTHOR_NAME:'Research Code Factory',GIT_AUTHOR_EMAIL:'research@localhost',GIT_COMMITTER_NAME:'Research Code Factory',GIT_COMMITTER_EMAIL:'research@localhost',PYTHONDONTWRITEBYTECODE:'1',OPENBLAS_NUM_THREADS:'2',OMP_NUM_THREADS:'2',MPLCONFIGDIR:'/tmp/spark-mpl'};
await mkdir(env.HOME,{recursive:true,mode:0o700});
if(!Number.isInteger(cfg.ownership_lease_seconds)||cfg.ownership_lease_seconds<3600||!Number.isInteger(cfg.ownership_heartbeat_seconds)||cfg.ownership_heartbeat_seconds<1||cfg.ownership_heartbeat_seconds*3>=cfg.ownership_lease_seconds)throw Error('Invalid renewable ownership lease configuration');
export async function atomic(path,value){
 const temp=path+'.tmp-'+process.pid;const f=await open(temp,'w',0o600);try{await f.writeFile(JSON.stringify(value,null,2)+'\n');await f.sync()}finally{await f.close()}await rename(temp,path);const d=await open(dirname(path),'r');try{await d.sync()}finally{await d.close()}
}
const save=(p,v)=>atomic(resolve(root,p),v);
let state;try{state=JSON.parse(await readFile(resolve(root,'status.json')))}catch(e){if(e.code!=='ENOENT')throw e;state={version:1,sequence:0,circuits:{},accepted:[],repair_count:0,base:cfg.initial_base,status:'starting',goal_status:'unresolved'}}
const policyHash=sha(JSON.stringify({cfg,goal}));
if(state.policy_hash&&state.policy_hash!==policyHash){
 const migration=JSON.parse(await readFile(resolve(root,'approved-policy-migration.json')));
 if(migration.from!==state.policy_hash||migration.to!==policyHash||!['remove-research-deadlines','lifecycle-recovery-repair'].includes(migration.kind))throw Error('Approved policy/goal changed; explicit migration required');
 if(migration.kind==='lifecycle-recovery-repair'){
  const previous=migration.prior_configuration;
  if(sha(JSON.stringify({cfg:previous,goal}))!==migration.from||JSON.stringify(Object.fromEntries(Object.entries(previous).filter(([k])=>!['controller','controller_sha256'].includes(k))))!==JSON.stringify(Object.fromEntries(Object.entries(cfg).filter(([k])=>!['controller','controller_sha256'].includes(k)))))throw Error('Lifecycle repair cannot broaden runtime permissions, models, scope or verification');
 }
 state.policy_migrations=[...(state.policy_migrations??[]),{...migration,applied_utc:now()}];
}
state.policy_hash=policyHash;
const circuitArgument=process.argv.indexOf('--pool-circuits');
if(circuitArgument!==-1){
 const shared=JSON.parse(await readFile(process.argv[circuitArgument+1]));
 for(const [model,circuit] of Object.entries(shared))if(!state.circuits[model]||circuit.until>state.circuits[model].until)state.circuits[model]=circuit;
}
async function persist(){state.updated_utc=now();await save('status.json',state)}
async function event(kind,detail={}){const entry={at:now(),...detail,kind};const f=await open(resolve(root,'events.ndjson'),'a',0o600);try{await f.writeFile(JSON.stringify(entry)+'\n');await f.sync()}finally{await f.close()}console.log(JSON.stringify(entry));if(['accepted','paused','all_free_unavailable'].includes(kind)){const p=spawn('/usr/bin/notify-send',[goal.id+' Code Factory',kind+': '+(detail.reason??detail.commit??'See status.json')],{stdio:'ignore'});p.on('error',()=>{})}}
const git=(cwd,...args)=>execFileSync('/usr/bin/git',['-c','core.hooksPath=/dev/null',...args],{cwd,env,encoding:'utf8',timeout:30000,maxBuffer:5e6}).trim();
const port=()=>new Promise(ok=>{const s=createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>ok(p))})});
function safe(v){if(Array.isArray(v))return v.filter(x=>!['thinking','reasoning'].includes(x?.type)).map(safe);if(v&&typeof v==='object')return Object.fromEntries(Object.entries(v).filter(([k,x])=>!(['thinking','reasoning'].includes(k)&&typeof x!=='number')).map(([k,x])=>[k,safe(x)]));return v}

async function runCycle(sequence){
 const dir=resolve(root,'cycles',String(sequence).padStart(6,'0'));await mkdir(dir,{recursive:true,mode:0o700});const profile=resolve(dir,'native');await mkdir(profile,{mode:0o700});
 for(const d of ['home','config','data','cache','state'])await mkdir(resolve(profile,d),{mode:0o700});
 const nativeEnv={...env,HOME:resolve(profile,'home'),XDG_CONFIG_HOME:resolve(profile,'config'),XDG_DATA_HOME:resolve(profile,'data'),XDG_CACHE_HOME:resolve(profile,'cache'),XDG_STATE_HOME:resolve(profile,'state')};
 const operator=randomBytes(32).toString('hex'),worker=randomBytes(32).toString('hex');let password='',native,controller,client,child,parent,work,job,lease,phase='catalog';
 let out={sequence,started_utc:now(),status:'starting',base:state.base,goal_id:goal.id,cleanup_confirmed:false};
 const cycleSave=async()=>{await atomic(resolve(dir,'result.json'),out);state.current={sequence,directory:dir,status:out.status,job,child:child?.id,worktree:work,phase};await persist()};
 const processes=[];
 const red=s=>s.replaceAll(operator,'[redacted]').replaceAll(worker,'[redacted]').replaceAll(password||'\0','[redacted]').split('\n').filter(l=>!/server password|authorization|bearer/i.test(l)).join('\n');
 async function launch(binary,args,vars,cwd,ready){const p=spawn(binary,args,{cwd,env:vars,stdio:['ignore','pipe','pipe']});processes.push(p);let output='';const append=b=>output=(output+b.toString()).slice(-1048576);p.stdout.on('data',append);p.stderr.on('data',append);p._log=()=>red(output);for(let i=0;i<300;i++){const found=ready(output);if(found)return{p,...found};if(p.exitCode!==null||p.signalCode!==null)throw Error('Process startup failed: '+red(output));if(shutdown)throw Error('Operator stopping');await delay(100)}throw Error('Process startup timeout')}
 async function stop(p){if(!p||p.exitCode!==null||p.signalCode!==null)return;const ended=new Promise(ok=>p.once('exit',ok));p.kill('SIGTERM');await Promise.race([ended,delay(8000)]);if(p.exitCode===null&&p.signalCode===null){p.kill('SIGKILL');await ended}}
 let controllerURL='http://127.0.0.1:'+await port(),nativeURL='http://127.0.0.1:'+await port();
 async function op(name,body={}){const res=await fetch(controllerURL+'/operator/'+name,{method:'POST',headers:{authorization:'Bearer '+operator,'content-type':'application/json'},body:JSON.stringify({job_id:job,...body}),signal:AbortSignal.timeout(40000)});const value=await res.json();await atomic(resolve(dir,'operator-'+name+'.json'),value);if(!res.ok)throw Error('Controller '+name+': '+JSON.stringify(value));return value}
 async function startNative(location,factory=false){
  const start=await launch('/usr/bin/bwrap',[...sandbox(cfg,repo,location,profile,protectedPaths),'serve','--hostname','127.0.0.1','--port',new URL(nativeURL).port],{...nativeEnv,...(factory?{PSTACK_FACTORY_URL:controllerURL,PSTACK_FACTORY_PROJECT:cfg.project_id??'spark-full-scroll',PSTACK_FACTORY_TOKEN:worker}:{})},location,o=>{const p=/server password (\S+)/.exec(o);return o.includes('server listening on')&&p?{password:p[1]}:undefined});native=start.p;password=start.password;
  client=OpenCode.make({baseUrl:nativeURL,fetch:(u,o={})=>fetch(u,{...o,signal:AbortSignal.any([AbortSignal.timeout(30000),...(o.signal?[o.signal]:[])])}),headers:{authorization:'Basic '+Buffer.from('opencode:'+password).toString('base64'),'x-opencode-directory':location}});
  if((await client.server.info()).version!=='2.0.29')throw Error('Unsupported native runtime');
 }
 try{
  await cycleSave();await startNative(repo);await delay(5000);
  const models=freeModels((await client.model.list()).data);await atomic(resolve(dir,'free-catalog.json'),{observed_utc:now(),runtime:'2.0.29',models});state.catalog=models;state.catalog_observed_utc=now();
  out.model=route(models,state.circuits,Date.now());await stop(native);await writeFile(resolve(dir,'catalog-server.log'),native._log());native=undefined;
  if(!out.model){out.status='all_free_unavailable';return out}
  const [providerID,...rest]=out.model.split('/'),modelID=rest.join('/');
  phase='admission';job=(cfg.job_prefix??'spark')+'-'+String(sequence).padStart(6,'0');out.job_id=job;
  const config={project_id:cfg.project_id??'spark-full-scroll',repository:repo,data_dir:resolve(dir,'controller'),delivery_dir:resolve(dir,'delivery'),integration_ref:cfg.integration_ref??'refs/heads/factory-integration',profiles:cfg.profiles,policy:{models:models.map(m=>m.id),free_only:true,spend_ceiling:0,data_approved:true,cooldown_seconds:60},max_workers:1,lease_seconds:cfg.ownership_lease_seconds,max_attempts:1,opencode_url:nativeURL,verifier_executable:cfg.controller};
  await atomic(resolve(dir,'controller.json'),config);
  controller=(await launch(cfg.controller,['serve','--config',resolve(dir,'controller.json'),'--listen',new URL(controllerURL).host],{...env,FACTORY_OPERATOR_TOKEN:operator},repo,o=>o.includes('controller listening on')?{}:undefined)).p;
  // Controller native auth is supplied after native start below, before binding.
  const standing=await op('standing-goal',{goal});out.standing_goal=standing;
  const admissionContext=await buildResearchContext({root,repo,state,goal,cfg,beforeSequence:sequence});
  const task=admissionContext.text;out.task=task;out.context_provenance=admissionContext.provenance;
  await atomic(resolve(dir,'research-context.json'),admissionContext);
  const template=JSON.parse(await readFile(resolve(root,'job-template.json')));await op('admit',{...template,job_id:job,base_commit:state.base,task_brief:task});
  const prepared=await op('prepare',{mode:'opencode_v2'});work=prepared.attempt.worktree;out.worktree=work;out.attempt=prepared.attempt.attempt_id;out.git_control_sha256=sha(await readFile(resolve(work,'.git')));lease=new OwnershipLease(()=>op('heartbeat',{attempt_id:out.attempt}),{intervalMs:cfg.ownership_heartbeat_seconds*1000}).start();await lease.tick(true);await cycleSave();
  if(git(work,'rev-parse','--show-toplevel')!==work)throw Error('Worker directory is not actual codebase root');
  await symlink(cfg.venv,resolve(work,'.venv'));await symlink(cfg.data,resolve(work,'data'));await mkdir(resolve(work,'.opencode/pstack-admission'),{recursive:true});
  await writeFile(resolve(work,'opencode.json'),JSON.stringify({plugins:[cfg.plugin],model:out.model,snapshots:false,share:'disabled',update:'disable'},null,2));
  // Fresh profile read, exact current prices are checked again by the native plugin per request.
  await startNative(work,true);
  // Restart dedicated controller once with the actual server password. DB/config unchanged.
  await stop(controller);controller=(await launch(cfg.controller,['serve','--config',resolve(dir,'controller.json'),'--listen',new URL(controllerURL).host],{...env,FACTORY_OPERATOR_TOKEN:operator,FACTORY_OPENCODE_PASSWORD:password},repo,o=>o.includes('controller listening on')?{}:undefined)).p;
  await atomic(resolve(dir,'host-private.json'),{operator,worker,password,controllerURL,nativeURL});
  await client.agent.list();for(let i=0;i<100;i++){if((await client.command.list()).data.some(c=>c.name==='pstack-factory-start'))break;await delay(100)}
  const p=(await client.plugin.list()).data.find(p=>p.id==='pstack.opencode');if(p?.state.status!=='active')throw Error('pstack plugin not active');out.plugin=p;
  const permissions=[{action:'*',resource:'*',effect:'deny'},{action:'pstack.workflow',resource:'feature',effect:'allow'},{action:'pstack.cancel',resource:'*',effect:'allow'},{action:'pstack.factory',resource:'*',effect:'allow'},{action:'read',resource:work+'/**',effect:'allow'},{action:'read',resource:'*',effect:'allow'},{action:'edit',resource:work+'/**',effect:'allow'},{action:'edit',resource:'*',effect:'allow'},{action:'shell',resource:'*',effect:'allow'}];
  // Kernel mount boundary permits writes only to this code worktree, private native profile and /tmp.
  // Existing tests and provenance are frozen by Factory's independent protected-path check.
  parent=await client.session.create({title:goal.id+' research cycle '+sequence,location:{directory:work},model:{providerID,id:modelID},permissions});out.parent=parent.id;
  await client.session.command({sessionID:parent.id,name:'pstack-prepare',text:'cycle-'+sequence+' feature '+task});
  child=(await client.session.list({parentID:parent.id})).data.find(s=>s.metadata?.pstack?.request==='cycle-'+sequence);if(!child||child.parentID!==parent.id||child.location.directory!==work)throw Error('Native ownership/cwd mismatch');
  out.child=child.id;await op('bind-v2',{session_id:child.id,worker_token:worker});await client.session.command({sessionID:child.id,name:'pstack-factory',text:'claim claim-'+sequence});
  phase='native';out.status='running';await cycleSave();await event('native_started',{sequence,model:out.model,worktree:work,job,session:child.id});
  await client.session.command({sessionID:parent.id,name:'pstack-factory-start',text:'cycle-'+sequence});
  child=await observeResearch({get:()=>client.session.get({sessionID:child.id}),checkpoint:async session=>{out.native_session=safe(session);await atomic(resolve(dir,'native-session.json'),safe(session));await cycleSave()},resources:async()=>{if(!await diskOK())throw Error('Disk resource limit reached')},lease,shouldStop:()=>shutdown,sleep:delay});
  const context=safe(await client.session.context({sessionID:child.id}));await atomic(resolve(dir,'native-context.json'),context);out.native_session=safe(await client.session.get({sessionID:child.id}));out.usage={cost:out.native_session.cost??null,tokens:out.native_session.tokens??null,model:out.native_session.model??null};
  if(shutdown)throw Error('Operator stopping');if(child.outcome!=='succeeded'){const errors=context.filter(m=>['assistant','idle'].includes(m.type)&&m.error).map(m=>m.error);throw Error(child.outcome?JSON.stringify(errors.length?errors:[{type:'native.failed',message:'Native outcome '+child.outcome}]):'Native execution ended without a terminal outcome')}
  phase='native_shells';await cycleSave();
  await awaitNativeShells({get:()=>client.shell.list({location:{directory:work}}),worktree:work,checkpoint:observation=>atomic(resolve(dir,'native-shells.json'),observation),resources:async()=>{if(!await diskOK())throw Error('Disk resource limit reached')},lease,shouldStop:()=>shutdown,sleep:delay});
  // Verification effects already have their own bounded trusted executor. Avoid heartbeat
  // revisions racing an in-flight receipt; renew before each verification stage.
  await lease.stop();await lease.tick(true);
  phase='verification';out.status='verifying';await cycleSave();
  // Commit only approved tracked/additional source paths; never ignored data/native configuration.
  const changed=git(work,'status','--porcelain','--untracked-files=all');out.diff_status=changed;
  if(!changed)throw Error('No implementation/evidence change; choose a concrete experiment or report a data blocker');
  if(sha(await readFile(resolve(work,'.git')))!==out.git_control_sha256)throw Error('Git ownership scope violation');
  if(!approvedScope(git(work,'diff','--name-only',cfg.initial_base).split('\n').filter(Boolean),template))throw Error('Protected/allowed path scope violation');
  git(work,'add','--',...template.allowed_paths.filter(p=>{try{execFileSync('/usr/bin/test',['-e',resolve(work,p)]);return true}catch{return false}}));
  if(!git(work,'diff','--cached','--name-only'))throw Error('No approved candidate changes');
  git(work,'commit','-m',goal.id+' research cycle '+sequence+' (independent verification required)');out.candidate=git(work,'rev-parse','HEAD');out.scope_approved=true;
  await client.session.command({sessionID:child.id,name:'pstack-factory',text:'verify verify-'+sequence+' '+out.candidate});await op('advance');
  let verified;while(!shutdown){lease.check();verified=await op('status');if(verified.active_verification_receipt)break;if(verified.state==='blocked')throw Error('Controller verification blocked: '+JSON.stringify(verified));await delay(1500)}
  out.candidate_status=verified;if(verified?.active_verification_receipt?.outcome!=='passed')throw Error('Independent verification failed: '+JSON.stringify(verified));
  await lease.tick(true);
  await client.session.command({sessionID:child.id,name:'pstack-factory',text:'submit submit-'+sequence+' Bounded research milestone only; standing scientific objective remains unresolved'});
  let done;while(!shutdown){lease.check();done=await op('status');if(done.state==='done')break;if(done.state==='blocked')throw Error('Controller integration blocked: '+JSON.stringify(done));await delay(1500)}
  if(done?.state!=='done'||done.active_verification_receipt?.outcome!=='passed')throw Error('Independent integration incomplete');
  if(sha(await readFile(done.delivery.destination))!==done.delivery.sha256)throw Error('Delivered archive hash mismatch');
  out.acceptance=done;out.status='accepted';await atomic(resolve(dir,'accepted.json'),done);
  try{out.scientific_blocker=(await readFile(resolve(work,'docs/BLOCKED.md'),'utf8')).slice(0,4000)}catch{}
 }catch(e){out.error=red(String(e)).slice(0,20000);out.failure_kind=out.error.includes('scope violation')?'blocked':classify(out.error,phase);out.status='failed';}
 finally{
  if(lease)await lease.stop();out.phase=phase;
  if(child&&client){try{const s=await client.session.get({sessionID:child.id});out.usage={model:s.model??null,cost:s.cost??null,tokens:s.tokens??null};out.native_session=safe(s);await atomic(resolve(dir,'native-session-final.json'),safe(s));await atomic(resolve(dir,'native-context-final.json'),safe(await client.session.context({sessionID:child.id})))}catch(e){out.accounting_error=red(String(e));out.usage??={cost:null,tokens:null,model:null}}}
  if(out.status!=='accepted'&&controller&&job){try{await op('cancel');out.abort=await op('abort');const s=await op('status');out.recovery=s.execution_recovery;if(out.usage)out.usage.accounting_status=s.execution_recovery?.accounting?.status??'unknown';out.execution_stop=s.execution_recovery?.execution_stop?.status??s.execution_recovery?.execution_stop;await atomic(resolve(dir,'fenced-status.json'),s)}catch(e){out.fence_error=red(String(e))}}
  for(const p of processes.toReversed())await stop(p);
  out.cleanup_confirmed=processes.every(p=>p.exitCode!==null||p.signalCode!==null);
  // Native bwrap has a private PID namespace: termination kills all namespace descendants.
  out.namespace_cleanup='bwrap private PID namespace; die-with-parent; host-owned TERM then KILL cleanup; foreground pool retains ownership lock';
  if(out.status!=='accepted'&&work&&out.cleanup_confirmed&&out.recovery?.execution_stop?.status==='confirmed'){
   try{
    const template=JSON.parse(await readFile(resolve(root,'job-template.json')));
    if(sha(await readFile(resolve(work,'.git')))!==out.git_control_sha256)throw Error('Git ownership scope violation');
    if(!approvedScope(git(work,'diff','--name-only',cfg.initial_base).split('\n').filter(Boolean),template))throw Error('Protected/allowed path scope violation');
    const delta=git(work,'diff','--stat');
    if(delta){
     // Unaccepted WIP remains explicit; only a later exact candidate receipt may accept it.
     await mkdir(resolve(work,'docs'),{recursive:true});
     await writeFile(resolve(work,'docs/FACTORY-RECOVERY.md'),'# Unaccepted stopped checkpoint\n\nCycle '+sequence+' ended without acceptance. Prior error: '+out.error.slice(0,1800)+'\n\nChanged files: \n'+delta+'\nNo verification or standing-goal success is implied. Repair and independently verify this checkpoint.\n');
     git(work,'add','--',...template.allowed_paths.filter(p=>{try{execFileSync('/usr/bin/test',['-e',resolve(work,p)]);return true}catch{return false}}));
     if(git(work,'diff','--cached','--name-only')){git(work,'commit','-m','Preserve unaccepted stopped research checkpoint '+sequence);out.checkpoint=git(work,'rev-parse','HEAD')}
    }
   }catch(e){out.checkpoint_error=red(String(e))}
  }

  for(let i=0;i<processes.length;i++)await writeFile(resolve(dir,'process-'+i+'.log'),processes[i]._log());
  out.ended_utc=now();await atomic(resolve(dir,'result.json'),out);
 }
 return out;
}
async function diskOK(){const free=Number(execFileSync('/usr/bin/df',['--output=avail','-B1',root],{encoding:'utf8'}).trim().split('\n').at(-1));const used=Number(execFileSync('/usr/bin/du',['-s','-B1',root],{encoding:'utf8',timeout:30000}).split(/\s/)[0]);state.resources={free_bytes:free,used_bytes:used,at:now()};return free>=cfg.min_free_bytes&&used<=cfg.max_state_bytes}
async function pause(reason){state.status='paused';state.pause_reason=reason;await persist();await event('paused',{reason});if(oneCycle){shutdown=true;return}while(!shutdown)await delay(1000)}
// Exact pinned binaries/plugin, no downloaded script execution or unattended upstream promotion.
if(sha(await readFile(cfg.native))!==cfg.native_sha256||sha(await readFile(cfg.controller))!==cfg.controller_sha256||git(cfg.plugin,'rev-parse','HEAD')!==cfg.plugin_commit||git(cfg.plugin,'status','--porcelain')!=='')throw Error('Pinned runtime/controller/plugin changed');
if(state.current){
 const prior=JSON.parse(await readFile(resolve(state.current.directory,'result.json')));
 if(recoveryAction(prior)==='pause'){await pause('Previous cycle lacks confirmed ownership/cleanup; inspect retained controller/native evidence before resume');process.exit(0)}
 if(prior.status==='accepted'){
  // Reconcile the existing receipt through the real controller after a crash at the host handoff.
  const privateState=JSON.parse(await readFile(resolve(state.current.directory,'host-private.json')));
  const endpoint='http://127.0.0.1:'+await port();const p=spawn(cfg.controller,['serve','--config',resolve(state.current.directory,'controller.json'),'--listen',new URL(endpoint).host],{cwd:repo,env:{...env,FACTORY_OPERATOR_TOKEN:privateState.operator,FACTORY_OPENCODE_PASSWORD:privateState.password},stdio:'ignore'});
  let saved;try{for(let i=0;i<100;i++){try{const r=await fetch(endpoint+'/operator/status',{method:'POST',headers:{authorization:'Bearer '+privateState.operator,'content-type':'application/json'},body:JSON.stringify({job_id:prior.job_id}),signal:AbortSignal.timeout(1000)});if(r.ok){saved=await r.json();break}}catch{}await delay(100)}
   if(saved?.state!=='done'||saved.integrated_commit!==prior.acceptance.integrated_commit||saved.active_verification_receipt?.verification_id!==prior.acceptance.active_verification_receipt.verification_id||sha(await readFile(saved.delivery.destination))!==saved.delivery.sha256)throw Error('Recovered acceptance mismatch');
   state.base=saved.integrated_commit;if(!state.accepted.some(a=>a.job===prior.job_id))state.accepted.push({job:prior.job_id,commit:state.base,receipt:saved.active_verification_receipt.verification_id});delete state.continuation;await event('acceptance_recovered',{job:prior.job_id,commit:state.base});
  }finally{p.kill('SIGTERM');await new Promise(ok=>p.once('exit',ok))}
 }
}
state.current=null;
if(cfg.resume_checkpoint&&!state.source_resume){
 const checkpoint=cfg.resume_checkpoint;
 if(state.sequence!==checkpoint.source_cycle||state.base!==checkpoint.prior_base||checkpoint.acceptance!=='none'||checkpoint.accounting!=='unknown')throw Error('Stopped checkpoint resume context changed; inspect before activation');
 for(const [path,digest] of Object.entries(checkpoint.files))if(sha(execFileSync('/usr/bin/git',['-C',repo,'show',checkpoint.commit+':'+path]))!==digest)throw Error('Stopped checkpoint content changed');
 state.base=checkpoint.commit;state.source_resume={...checkpoint,applied_utc:now()};
 delete state.continuation;
 await persist();await event('unaccepted_checkpoint_resumed',{commit:checkpoint.commit,acceptance:'none',prior_accounting:'unknown'});
}
if(state.status==='paused')await pause(state.pause_reason??'Operator review required');
while(!shutdown){
 if(await access(resolve(root,'PAUSE')).then(()=>true,()=>false)){await pause('Operator PAUSE file');break}
 if(!await diskOK()){await pause('Disk resource limit reached; no scientific artifacts deleted');break}

 state.sequence++;state.status='running';await persist();const out=await runCycle(state.sequence);
 if(out.status==='accepted'||out.status==='all_free_unavailable'||replacementAllowed(out))state.current=null;state.last_cycle={sequence:out.sequence,status:out.status,result:resolve(root,'cycles',String(out.sequence).padStart(6,'0'),'result.json')};
 if(out.status==='accepted'){
  state.base=out.acceptance.integrated_commit;state.accepted.push({job:out.job_id,commit:state.base,receipt:out.acceptance.active_verification_receipt.verification_id});state.repair_count=0;delete state.continuation;await event('accepted',{sequence:out.sequence,commit:state.base,goal_status:'unresolved'});if(out.scientific_blocker){delete state.continuation;await event('scientific_limitation',{sequence:out.sequence,claim:out.scientific_blocker.slice(0,1200),goal_status:'unresolved'});}
 }else if(out.status==='all_free_unavailable'){
  state.status='all_free_unavailable';state.next_check_utc=new Date(Date.now()+cfg.unavailable_backoff_seconds*1000).toISOString();await persist();await event('all_free_unavailable',{reason:'No advertised compatible free model outside open circuits; bounded catalog recheck'});
  if(oneCycle)break;
  for(let i=0;i<cfg.unavailable_backoff_seconds&&!shutdown;i++)await delay(1000);continue;
 }else{
  // Never retry an uncertain execution. Unknown accounting stays unknown in immutable cycle evidence.
  // A fresh independently admitted job can follow an owned, positively stopped worker; no old grant/receipt is reused.
  if(out.recovery?.accounting?.status==='violating'){await pause('Known accounting/model policy violation; no automatic fallback: '+out.error);break}
  if(!replacementAllowed(out)){await pause('Execution/recovery evidence incomplete: '+(out.error??'unknown'));break}
  const stop=out.recovery.execution_stop;const confirmed=stop==='confirmed'||stop?.status==='confirmed';
  if(!confirmed){await pause('Controller could not positively confirm native execution stopped: '+JSON.stringify(stop));break}
  if(['denied','unavailable','transient'].includes(out.failure_kind)){if(out.checkpoint){state.base=out.checkpoint;delete state.continuation;}state.circuits=failure(state.circuits,out.model,out.failure_kind,Date.now());await event('model_circuit',{model:out.model,failure_kind:out.failure_kind,usage:out.usage??null});}
  else if(out.failure_kind==='repair'){
   state.repair_count++;
   // Retain unaccepted candidate as repair input, not as an accepted/integrated milestone.
   if(out.checkpoint||(out.candidate&&out.scope_approved))state.base=out.checkpoint??out.candidate;
   delete state.continuation;
   await event('repair_required',{sequence:out.sequence,model:out.model,candidate:out.candidate??null});
  }else{await pause(out.error??'Unknown non-provider failure');break}
 }
 state.status=shutdown?'stopped':'waiting';await persist();if(oneCycle)break;for(let i=0;i<cfg.between_cycles_seconds&&!shutdown;i++)await delay(1000);
}
if(!oneCycle&&state.status!=='paused')state.status='stopped';await persist();await event('stopped');
