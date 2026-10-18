#!/usr/bin/env node
// Shipped release regression for native execution custody. Real Linux
// subreaper/pidfd supervisor process trees over the existing control pipe with
// a delayed descendant writer, independent start/stop identity and foreign
// receipt rejection, actual bwrap read-only source mounts with a writable
// native profile and admission metadata, credential-environment exclusion and
// changed Git symlink rejection. Every observation comes from a real process,
// mount or Git object; no model or provider calls and no self-asserted
// booleans. Missing facilities are failures, never skips.
import {execFileSync} from 'node:child_process';
import {existsSync,constants} from 'node:fs';
import {mkdtemp,mkdir,writeFile,readFile,access,rm,symlink} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
import {
  startNativeExecution,stopNativeExecution,nativeExecutionBinding,
  nativeEnvironment,assertImmutableSourceCandidate,freezeNativeSource,
} from './native-execution.mjs';
import {sandbox} from './spark-sandbox.mjs';

const checks=[];
const ok=name=>checks.push(name);
const sleep=ms=>new Promise(done=>setTimeout(done,ms));
const sha=b=>createHash('sha256').update(b).digest('hex');
const controller=process.env.FACTORY_FIXTURE_CONTROLLER??resolve(import.meta.dirname,'../target/debug/code-factory-controller');
const commit='ddd247adecfd1881a457bda6b6483dd85c129c3d';
// Fail closed before any evidence when a required real facility is absent.
for(const facility of [controller,'/usr/bin/bwrap','/usr/bin/python3','/usr/bin/git'])
  await access(facility,constants.X_OK).catch(e=>{throw Error('Required release facility unavailable: '+facility+' ('+e.code+')')});
const git=(cwd,args)=>execFileSync('/usr/bin/git',['-c','core.hooksPath=/dev/null',...args],{cwd,encoding:'utf8',timeout:30000}).trim();
const commitAs=(cwd,message)=>git(cwd,['-c','user.name=Release Fixture','-c','user.email=release@localhost','commit','-q','-m',message]);

// 1. Credential-environment exclusion is an allowlist, checked both directions.
const secrets={GITHUB_TOKEN:'gh_fixture_secret_value',AWS_SECRET_ACCESS_KEY:'aws_fixture_secret_value',AGENT_PRIVATE_TOKEN:'agent_fixture_secret_value'};
const factoryToken='pstack_fixture_worker_token';
const vars=nativeEnvironment({PATH:'/usr/bin:/bin',HOME:'/home/fixture',...secrets,PSTACK_FACTORY_TOKEN:factoryToken});
assert.deepEqual(Object.keys(vars).sort(),['HOME','PATH','PSTACK_FACTORY_TOKEN']);
assert.equal(vars.PSTACK_FACTORY_TOKEN,factoryToken);
ok('native_environment_allowlist_excludes_credentials');

// 2. Only the single owned writable-source bind is tightened to read-only.
const ownedBind=['--unshare-user','--bind','/work','/work','--bind','/profile','/profile'];
assert.deepEqual(freezeNativeSource(ownedBind,'/work'),['--unshare-user','--ro-bind','/work','/work','--bind','/profile','/profile']);
assert.throws(()=>freezeNativeSource(['--unshare-user'],'/work'),/ownership unknown/);
assert.throws(()=>freezeNativeSource(['--bind','/work','/work','--bind','/work','/work'],'/work'),/ownership unknown/);
ok('freeze_source_requires_exactly_one_owned_bind');

// 3. Changed Git source symlinks are rejected against the approved base,
//    including a changed file type, while ordinary work and removal pass.
const handles=[];
try{
 const workRoot=await mkdtemp(resolve(tmpdir(),'release-native-work-'));
 try{
  const gitRoot=resolve(workRoot,'repository');
  await mkdir(gitRoot);
  git(gitRoot,['init','-q']);
  await writeFile(resolve(gitRoot,'base.txt'),'base\n');
  await writeFile(resolve(gitRoot,'regular.txt'),'regular\n');
  git(gitRoot,['add','base.txt','regular.txt']);
  commitAs(gitRoot,'release fixture base');
  const base=git(gitRoot,['rev-parse','HEAD']);
  const staged=()=>git(gitRoot,['diff','--cached',base,'--raw','--no-abbrev','--no-renames']);
  await symlink('../../etc/passwd',resolve(gitRoot,'native-alias'));
  git(gitRoot,['add','native-alias']);
  const newSymlink=staged();
  assert.match(newSymlink,/^:000000 120000 /m,'git raw format for a new symlink changed');
  assert.throws(()=>assertImmutableSourceCandidate(newSymlink),/symlink/);
  git(gitRoot,['reset','-q']);
  await writeFile(resolve(gitRoot,'base.txt'),'ordinary work\n');
  git(gitRoot,['add','base.txt']);
  assert.doesNotThrow(()=>assertImmutableSourceCandidate(staged()));
  git(gitRoot,['reset','-q']);
  assert.equal(staged(),'','no staged change must stay accepted');
  assert.doesNotThrow(()=>assertImmutableSourceCandidate(staged()));
  git(gitRoot,['add','native-alias']);
  commitAs(gitRoot,'release fixture carries a source alias');
  const aliasBase=git(gitRoot,['rev-parse','HEAD']);
  git(gitRoot,['rm','-q','native-alias']);
  const removed=git(gitRoot,['diff','--cached',aliasBase,'--raw','--no-abbrev','--no-renames']);
  assert.match(removed,/^:120000 000000 /m,'git raw format for a removed symlink changed');
  assert.doesNotThrow(()=>assertImmutableSourceCandidate(removed));
  git(gitRoot,['rm','-f','regular.txt']);
  await symlink('/etc/passwd',resolve(gitRoot,'regular.txt'));
  git(gitRoot,['add','regular.txt']);
  const changed=git(gitRoot,['diff','--cached',aliasBase,'--raw','--no-abbrev','--no-renames']);
  assert.match(changed,/^:100644 120000 /m,'git raw format for a changed file type changed');
  assert.throws(()=>assertImmutableSourceCandidate(changed),/symlink/);
  ok('git_changed_source_symlink_rejected_against_base');

  // 4. Actual kernel mounts: source read-only only after freezeNativeSource,
  //    with the native profile and admission metadata writable in both runs.
  const repo=resolve(workRoot,'mount-repository'),mountWork=resolve(workRoot,'mount-work'),profile=resolve(workRoot,'native-profile');
  for(const p of [repo,resolve(repo,'.git'),mountWork,profile,resolve(mountWork,'.opencode/pstack-admission')])await mkdir(p,{recursive:true});
  for(const p of ['.git','opencode.json','frozen.py'])await writeFile(resolve(mountWork,p),'frozen\n');
  await writeFile(resolve(workRoot,'operator-secret'),'outside-boundary');
  const cfg={plugin:resolve(import.meta.dirname),native:'/usr/bin/python3'};
  const probe=`import errno,os,pathlib,json
work=pathlib.Path(${JSON.stringify(mountWork)})
profile=pathlib.Path(${JSON.stringify(profile)})
admission=work/'.opencode/pstack-admission'
def blocked(path):
 try:
  path.write_text('release-probe')
  return False
 except OSError as error:
  return error.errno in (errno.EROFS,errno.EACCES,errno.EPERM)
admission.mkdir(exist_ok=True)
(profile/'owned').write_text('native-profile')
(admission/'marker').write_text('admission-metadata')
print(json.dumps({
 'source_file_readonly':blocked(work/'frozen.py'),
 'source_tree_readonly':blocked(work/'release-probe.txt'),
 'worktree_git_readonly':blocked(work/'.git'),
 'profile_writable':(profile/'owned').read_text()=='native-profile',
 'admission_writable':(admission/'marker').read_text()=='admission-metadata',
 'source_mount_readonly':(os.statvfs(work).f_flag & os.ST_RDONLY)!=0,
 'profile_mount_writable':(os.statvfs(profile).f_flag & os.ST_RDONLY)==0,
 'admission_mount_writable':(os.statvfs(admission).f_flag & os.ST_RDONLY)==0,
 'operator_secret_hidden':not pathlib.Path(${JSON.stringify(resolve(workRoot,'operator-secret'))}).exists(),
}))`;
  const runBwrap=mounts=>{
    const command=[...mounts];
    assert.equal(command.pop(),cfg.native,'sandbox command tail changed');
    return JSON.parse(execFileSync('/usr/bin/bwrap',[...command,'/usr/bin/python3','-c',probe],{encoding:'utf8',timeout:30000}));
  };
  const raw=sandbox(cfg,repo,mountWork,profile,['frozen.py']);
  // Negative control: the unfrozen builder keeps the source writable, so the
  // probe can distinguish a real read-only mount from an empty assertion.
  const open=runBwrap(raw);
  // Protected paths (.git, opencode.json, frozen.py) stay individually
  // read-only in both runs; only the freeze must close the source tree itself.
  for(const [name,value] of Object.entries({source_file_readonly:true,source_tree_readonly:false,worktree_git_readonly:true,source_mount_readonly:false,profile_writable:true,admission_writable:true,profile_mount_writable:true,admission_mount_writable:true,operator_secret_hidden:true}))
    assert.equal(open[name],value,'unfrozen mount '+name);
  const frozen=freezeNativeSource(raw,mountWork);
  const bindAt=raw.findIndex((_,i)=>raw[i]==='--bind'&&raw[i+1]===mountWork&&raw[i+2]===mountWork);
  assert.ok(bindAt>0,'sandbox source bind missing');
  assert.deepEqual(frozen,raw.map((_,i)=>i===bindAt?'--ro-bind':raw[i]),'freeze must change only the owned source mount');
  const locked=runBwrap(frozen);
  for(const [name,value] of Object.entries({source_file_readonly:true,source_tree_readonly:true,worktree_git_readonly:true,source_mount_readonly:true,profile_writable:true,admission_writable:true,profile_mount_writable:true,admission_mount_writable:true,operator_secret_hidden:true}))
    assert.equal(locked[name],value,'frozen mount '+name);
  ok('bwrap_readonly_source_with_writable_profile_and_admission');

  // 5. Real owned supervisor runs. Each execution drives the shipped
  //    startNativeExecution/stopNativeExecution custody path end to end.
  const work=resolve(workRoot,'execution-work');await mkdir(work);
  const home=resolve(workRoot,'execution-home');await mkdir(home);
  const scenarioEnv={PATH:'/usr/bin:/bin',HOME:home,...secrets,PSTACK_FACTORY_TOKEN:factoryToken};
  const spawnExecution=async({code,onProcess})=>startNativeExecution({
    controller,argv:['/usr/bin/python3','-c',code],env:scenarioEnv,cwd:work,directory:workRoot,commit,
    ready:o=>o.includes('release-native-ready')?{}:undefined,
    shouldStop:()=>false,onProcess:p=>{handles.push(p);onProcess?.(p)},
  });

  // 5a. A foreign start observation must fail closed even though the real
  //     supervisor produced the record; the owner then stops the live tree.
  const tamperDone=resolve(workRoot,'scenario-a-tamper-done');
  const codeA=`import pathlib,sys,time
marker=pathlib.Path(${JSON.stringify(tamperDone)})
for _ in range(600):
    if marker.exists(): break
    time.sleep(0.01)
else:
    sys.exit(3)
print('release-native-ready',flush=True)
time.sleep(30)`;
  let handleA=null,tamper=null;
  const startA=spawnExecution({code:codeA,onProcess:p=>{handleA=p;tamper=(async()=>{
    const startedPath=p._ownedNative.startedPath;
    for(let i=0;i<1000;i++){
      try{
        const started=JSON.parse(await readFile(startedPath,'utf8'));
        started.execution_id='f'.repeat(32);
        await writeFile(startedPath,JSON.stringify(started));
        await writeFile(tamperDone,'tampered');
        return;
      }catch(e){if(e.code!=='ENOENT')throw e}
      await sleep(10);
    }
    throw Error('start snapshot never appeared');
  })();tamper.catch(()=>{})}});
  await assert.rejects(startA,/ownership observation unknown/);
  await tamper;
  assert(handleA,'supervisor handle must be retained before readiness');
  assert.equal(JSON.parse(await readFile(handleA._ownedNative.startedPath,'utf8')).execution_id,'f'.repeat(32),'start snapshot tamper did not take effect');
  const stopA=await stopNativeExecution(handleA);
  assert.equal(stopA.receipt.termination_reason,'controller_disconnected');
  assert.equal(stopA.receipt.root_exit_status,-1,'stop must take effect while the owned root is alive');
  assert.equal(stopA.receipt.quiescent,true);
  assert.equal(stopA.receipt.leaked_descendants,false);
  ok('foreign_start_observation_rejected_and_live_tree_stopped');

  // 5b. Closing the control pipe stops a live tree whose delayed descendant
  //     writer has not run yet: the kernel receipt proves full reaping and
  //     the pending write never happens after the stop.
  const delayedB=resolve(workRoot,'scenario-b-delayed-writer.txt');
  const writerB='import time,pathlib;time.sleep(6);pathlib.Path('+JSON.stringify(delayedB)+').write_text("late-write")';
  const codeB=`import subprocess
child=subprocess.Popen(['/usr/bin/python3','-c',${JSON.stringify(writerB)}])
print('release-native-ready',flush=True)
child.wait()`;
  const startedB=await spawnExecution({code:codeB});
  const bindingB=nativeExecutionBinding(startedB.p);
  assert.equal(bindingB.execution_id,startedB.p._ownedNative.id);
  assert.equal(bindingB.plan_sha256,sha(await readFile(startedB.p._ownedNative.planPath,'utf8')));
  assert.equal(bindingB.started.supervisor.pid,startedB.p.pid);
  assert.equal(bindingB.started.supervisor.boot_id,(await readFile('/proc/sys/kernel/random/boot_id','utf8')).trim());
  assert.ok(Number.isInteger(bindingB.started.root_pid));
  ok('independent_start_identity_bound_to_live_supervisor');
  const stopB=await stopNativeExecution(startedB.p);
  assert.equal(stopB.execution_id,bindingB.execution_id);
  assert.equal(stopB.receipt.termination_reason,'controller_disconnected');
  assert.equal(stopB.receipt.root_exit_status,-1);
  assert.ok(stopB.receipt.descendants_reaped>=1,'delayed descendant must be reaped by the owner');
  assert.equal(stopB.receipt.quiescent,true);
  assert.equal(stopB.receipt.leaked_descendants,false);
  assert.equal(stopB.receipt.cleanup_error,null);
  assert.equal(stopB.receipt.supervisor.start_ticks,bindingB.started.supervisor.start_ticks);
  assert.equal(stopB.receipt.root_pid,bindingB.started.root_pid);
  await sleep(300);
  assert.equal(existsSync(delayedB),false,'stopped descendant writer must not write after its stop');
  ok('control_pipe_stop_kills_pending_delayed_descendant_writer');

  // 5c. Natural completion: the delayed descendant writer is orphaned to the
  //     Linux subreaper while the root is alive, really writes, and must be
  //     counted in the independent receipt. The process environment excludes
  //     credentials, and every foreign receipt variant is rejected before an
  //     owner recheck can pass.
  const delayedC=resolve(workRoot,'scenario-c-delayed-writer.txt');
  const environC=resolve(workRoot,'scenario-c-environ.json');
  const writerC='import time,pathlib;time.sleep(1.2);pathlib.Path('+JSON.stringify(delayedC)+').write_text("delayed-descendant-write")';
  const orphanC='import subprocess,sys;subprocess.Popen(["/usr/bin/python3","-c",'+JSON.stringify(writerC)+']);sys.exit(0)';
  const codeC=`import json,os,pathlib,subprocess,time
child=subprocess.Popen(['/usr/bin/python3','-c',${JSON.stringify(orphanC)}])
pathlib.Path(${JSON.stringify(environC)}).write_text(json.dumps(dict(os.environ)))
print('release-native-ready',flush=True)
time.sleep(2.5)`;
  const startedC=await spawnExecution({code:codeC});
  const bindingC=nativeExecutionBinding(startedC.p);
  assert.equal(bindingC.plan_sha256,sha(await readFile(startedC.p._ownedNative.planPath,'utf8')));
  for(let i=0;i<400&&startedC.p.exitCode===null&&startedC.p.signalCode===null;i++)await sleep(100);
  assert.equal(startedC.p.exitCode,0,'supervisor must reach natural quiescence without a stop request');
  const stopC=await stopNativeExecution(startedC.p);
  assert.equal(stopC.receipt.termination_reason,'root_exit');
  assert.equal(stopC.receipt.root_exit_status,0);
  assert.ok(stopC.receipt.descendants_reaped>=1,'delayed descendant must appear in the independent receipt');
  assert.equal(stopC.receipt.quiescent,true);
  assert.equal(stopC.receipt.leaked_descendants,false);
  assert.equal(stopC.receipt.cleanup_error,null);
  assert.equal(stopC.receipt.output_truncated,false);
  assert.equal(stopC.receipt.execution_id,bindingC.execution_id);
  assert.equal(stopC.receipt.supervisor.start_ticks,bindingC.started.supervisor.start_ticks);
  assert.equal(stopC.receipt.root_pid,bindingC.started.root_pid);
  assert.equal(await readFile(delayedC,'utf8'),'delayed-descendant-write');
  ok('natural_completion_delayed_descendant_writer_receipt');
  const environ=JSON.parse(await readFile(environC,'utf8'));
  for(const key of Object.keys(secrets))assert.equal(key in environ,false,key+' leaked into the owned process environment');
  assert.equal(environ.PATH,'/usr/bin:/bin');
  assert.equal(environ.PSTACK_FACTORY_TOKEN,factoryToken);
  const planBytes=await readFile(startedC.p._ownedNative.planPath,'utf8');
  for(const value of Object.values(secrets))assert.equal(planBytes.includes(value),false,value+' leaked into the execution plan');
  assert.equal(planBytes.includes('PSTACK_FACTORY_TOKEN='+factoryToken),true,'scoped factory credential must reach the native process');
  ok('credential_environment_exclusion_observed_in_real_process');
  const receiptPath=startedC.p._ownedNative.receiptPath;
  const realBytes=await readFile(receiptPath);
  const real=JSON.parse(realBytes);
  for(const [label,tamperedReceipt,rejected] of [
    ['foreign execution id',{...real,execution_id:'e'.repeat(32)},/quiescence unconfirmed/],
    ['foreign supervisor pid',{...real,supervisor:{...real.supervisor,pid:real.supervisor.pid+1}},/quiescence unconfirmed/],
    ['foreign root pid',{...real,root_pid:real.root_pid+1},/stop identity changed/],
  ]){
    await writeFile(receiptPath,JSON.stringify(tamperedReceipt));
    await assert.rejects(stopNativeExecution(startedC.p),rejected,label);
  }
  await writeFile(receiptPath,realBytes);
  assert.deepEqual((await stopNativeExecution(startedC.p)).receipt,real,'owner recheck must pass on the restored independent receipt');
  ok('foreign_receipts_rejected_and_owner_recheck_preserved');
 }finally{
  // Any failure above must still stop every retained supervisor handle.
  for(const p of handles)if(p.exitCode===null&&p.signalCode===null){
    try{await stopNativeExecution(p)}catch(e){p.kill('SIGKILL')}
  }
  await rm(workRoot,{recursive:true,force:true});
 }
}finally{
 for(const p of handles)if(p.exitCode===null&&p.signalCode===null)p.kill('SIGKILL');
}
console.log(JSON.stringify({
  passed:true,
  executed_checks:checks.length,
  checks,
  evidence:'real Linux subreaper/pidfd supervisor receipts, actual bwrap kernel mounts, real Git source custody; no model or provider calls',
  controller,
},null,2));
