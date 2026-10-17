import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {buildResearchContext,createCycleDirectory,commitSubject,loadGoalEvaluation,captureVerificationDiagnostics} from './research-context.mjs';
const sha=b=>createHash('sha256').update(b).digest('hex');
async function fixture(t){
 const root=await mkdtemp(join(tmpdir(),'factory-context-'));t.after(()=>rm(root,{recursive:true,force:true}));
 const repo=join(root,'repo');await mkdir(repo);const git=(...args)=>execFileSync('git',['-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false','-c','user.name=Test','-c','user.email=test@localhost',...args],{cwd:repo,encoding:'utf8'}).trim();
 git('init','-q');await mkdir(join(repo,'docs'));await writeFile(join(repo,'docs/CONTINUATION.md'),'## Cycle 7\nLATEST RESEARCH\n## Cycle 4\nOBSOLETE DIRECTIVE');
 git('add','.');git('commit','-qm','first');const first=git('rev-parse','HEAD');
 await writeFile(join(repo,'result.py'),'x=1\n');git('add','.');git('commit','-qm','second');const second=git('rev-parse','HEAD');
 await writeFile(join(root,'task.txt'),'Keep protected tests.');
 const goal={id:'open-research',objective:'Discover the result',completion_criteria:['Independent full evaluation'],pause_conditions:['Missing inputs']};
 const statuses={};
 const input={root,repo,goal,state:{base:first,continuation:'OBSOLETE CACHED CYCLE 4',last_cycle:{sequence:1}},cfg:{controller:'unused',initial_continuation:'INITIAL ONLY'},inspect:(_controller,config)=>statuses[config]};
 async function cycle(n,commit,accepted=true){
  const dir=join(root,'cycles',String(n).padStart(6,'0'));await mkdir(dir,{recursive:true});
  const archive=join(dir,'delivery');await writeFile(archive,'archive '+commit);
  const receipt={verification_id:'receipt-'+n,candidate_commit:commit,outcome:accepted?'passed':'failed',environment:{checks:[{check_id:'cpu',verified:accepted,exit_status:accepted?0:1,observations:{executed:9,failed:accepted?0:2}}]}};
  const status={state:accepted?'done':'blocked',state_revision:n,candidate_commit:commit,integrated_commit:accepted?commit:null,active_verification_receipt:receipt,delivery:accepted?{destination:archive,sha256:sha(await readFile(archive))}:null,execution_recovery:{execution_stop:{status:'confirmed'},accounting:{status:'settled'}}};
  const record={sequence:n,ended_utc:'now',cleanup_confirmed:true,recovery:status.execution_recovery,status:accepted?'accepted':'failed',job_id:'job-'+n,candidate:commit,acceptance:accepted?status:undefined};
  await writeFile(join(dir,'result.json'),JSON.stringify(record));statuses[join(dir,'controller.json')]=status;return {dir,status,record};
 }
 return {root,repo,git,first,second,input,cycle,statuses};
}
test('newest-first Markdown and stale cache never become next-step directives',async t=>{
 const f=await fixture(t);await f.cycle(7,f.second);f.input.state.base=f.second;
 const context=await buildResearchContext(f.input);
 assert.match(context.text,new RegExp(f.second));assert.match(context.text,/receipt-7/);assert.match(context.text,/CONTINUATION.md/);
 assert.doesNotMatch(context.text,/OBSOLETE|LATEST RESEARCH|INITIAL ONLY/);assert.match(context.text,/unresolved/);
 assert.match(context.text,/Use repository-relative paths/);assert.match(context.text,/do not retry that rejected path/);
 assert.equal(context.provenance.legacy_continuation_ignored,true);
});
test('repeated admission and restart discover latest completed cycle despite stale last_cycle',async t=>{
 const f=await fixture(t);await f.cycle(1,f.first);const a=await buildResearchContext(f.input);
 await f.cycle(2,f.second);f.input.state.base=f.second;const b=await buildResearchContext(f.input);
 assert.notEqual(a.provenance.context_sha256,b.provenance.context_sha256);assert.equal(b.provenance.sources[0].sequence,2);
 const restarted=await buildResearchContext({...f.input,state:{base:f.second,continuation:a.text,last_cycle:{sequence:1}}});assert.equal(restarted.text,b.text);
});
test('failed candidate stays explicitly unaccepted with useful structured test counts',async t=>{
 const f=await fixture(t);await f.cycle(1,f.first);await f.cycle(2,f.second,false);f.input.state.base=f.second;
 const {text}=await buildResearchContext(f.input);assert.match(text,/UNACCEPTED input/);assert.match(text,/"failed":2/);assert.match(text,/receipt-2/);assert.match(text,/"stop":"confirmed"/);
});
test('failed component log reaches repair admission and restart as untrusted receipt-bound feedback',async t=>{
 const f=await fixture(t),c=await f.cycle(1,f.second,false);f.input.state.base=f.second;
 const receipt=c.status.active_verification_receipt;
 const text="AssertionError: test_main_controller: 'awaiting_ci' != 'confirmed'";
 const artifact={reference:'artifact:independent-check.log',sha256:sha(text)};
 receipt.artifacts=[artifact];receipt.environment.checks[0].evidence=artifact;
 let requested=0;
 c.record.verification_diagnostics=await captureVerificationDiagnostics(c.status,async ref=>{
  requested++;assert.equal(ref,artifact.reference);return {sha256:sha(text),text,truncated:false};
 });
 await writeFile(join(c.dir,'result.json'),JSON.stringify(c.record));
 const context=await buildResearchContext(f.input);
 assert.equal(requested,1);assert.match(context.text,/test_main_controller/);assert.match(context.text,/UNACCEPTED input/);
 assert.equal((await buildResearchContext(f.input)).text,context.text);
 c.record.verification_diagnostics[0].text='forged';await writeFile(join(c.dir,'result.json'),JSON.stringify(c.record));
 await assert.rejects(buildResearchContext(f.input),/diagnostic receipt\/text mismatch/);
});
test('diagnostic source rejects foreign hashes and cannot grant acceptance or permissions',async t=>{
 const f=await fixture(t),c=await f.cycle(1,f.second,false),receipt=c.status.active_verification_receipt;
 const artifact={reference:'artifact:check.log',sha256:'a'.repeat(64)};
 receipt.artifacts=[artifact];receipt.environment.checks[0].evidence=artifact;
 await assert.rejects(captureVerificationDiagnostics(c.status,async()=>({sha256:'b'.repeat(64),text:'wrong log'})),/identity\/bound mismatch/);
 const rows=await captureVerificationDiagnostics(c.status,async()=>({sha256:artifact.sha256,text:'界'.repeat(9000)+'\u202eATTACK\x00',truncated:true}));
 assert.ok(Buffer.byteLength(rows[0].text)<=1000);assert.doesNotMatch(rows[0].text,/[\x00\u202e]/);
 c.record.verification_diagnostics=rows;rows[0].candidate=f.first;await writeFile(join(c.dir,'result.json'),JSON.stringify(c.record));
 await assert.rejects(buildResearchContext({...f.input,state:{base:f.second}}),/diagnostic receipt\/text mismatch/);
 receipt.outcome='passed';assert.deepEqual(await captureVerificationDiagnostics(c.status,()=>{throw Error('must not read')}),[]);
});
test('mutable worktree planning and document ordering do not affect immutable context',async t=>{
 const f=await fixture(t);await f.cycle(1,f.first);const a=await buildResearchContext(f.input);
 await writeFile(join(f.repo,'docs/CONTINUATION.md'),'MUTABLE FORGED INSTRUCTIONS');
 assert.equal((await buildResearchContext(f.input)).text,a.text);
});
test('stale base cannot silently repeat an older accepted cycle',async t=>{
 const f=await fixture(t);await f.cycle(1,f.first);await f.cycle(2,f.second);
 await assert.rejects(buildResearchContext(f.input),/predates or diverges/);
});
test('forged receipt identity and changed delivered archive fail closed',async t=>{
 const f=await fixture(t);const c=await f.cycle(1,f.first);c.status.active_verification_receipt.candidate_commit=f.second;
 await assert.rejects(buildResearchContext(f.input),/candidate mismatch/);
 c.status.active_verification_receipt.candidate_commit=f.first;await writeFile(c.status.delivery.destination,'tampered');
 await assert.rejects(buildResearchContext(f.input),/acceptance\/delivery mismatch/);
});
test('missing authoritative controller evidence or incomplete previous ownership blocks admission',async t=>{
 const f=await fixture(t);const c=await f.cycle(1,f.first);f.input.inspect=()=>{throw Error('controller unavailable')};await assert.rejects(buildResearchContext(f.input),/controller unavailable/);
 await writeFile(join(c.dir,'result.json'),JSON.stringify({...c.record,ended_utc:null}));await assert.rejects(buildResearchContext(f.input),/no terminal evidence/);
});
test('initial guidance only applies without history; required goal/instructions never silently truncate',async t=>{
 const f=await fixture(t);assert.match((await buildResearchContext(f.input)).text,/INITIAL ONLY/);
 f.input.goal.objective='X'.repeat(8000);await assert.rejects(buildResearchContext(f.input),/Required goal/);
 f.input.goal.objective='Small';await writeFile(join(f.root,'task.txt'),'Y'.repeat(8000));await assert.rejects(buildResearchContext(f.input),/Project instructions/);
});
test('in-progress current cycle is excluded and accepted handoff must reconcile before replacement',async t=>{
 const f=await fixture(t);await f.cycle(1,f.first);const c=await f.cycle(2,f.second);await writeFile(join(c.dir,'result.json'),JSON.stringify({...c.record,status:'running',ended_utc:null}));
 assert.match((await buildResearchContext({...f.input,beforeSequence:2})).text,/receipt-1/);
 await writeFile(join(c.dir,'result.json'),JSON.stringify({...c.record,status:'failed'}));f.input.state.base=f.second;
 await assert.rejects(buildResearchContext(f.input),/Unreconciled accepted cycle/);
});

test('restart refuses uncertain ownership even with a completed result and stale clean cache',async t=>{
 const f=await fixture(t);const c=await f.cycle(1,f.first,false);
 await writeFile(join(c.dir,'result.json'),JSON.stringify({...c.record,cleanup_confirmed:false}));
 await assert.rejects(buildResearchContext(f.input),/confirmed ownership/);
});
test('saved not-admitted flags cannot bypass independent recovery audit',async t=>{
 const f=await fixture(t),c=await f.cycle(1,f.first,false);
 await writeFile(join(c.dir,'result.json'),JSON.stringify({...c.record,cleanup_confirmed:false,_not_admitted:{kind:'not_admitted'}}));
 await assert.rejects(buildResearchContext(f.input),/confirmed ownership/);
});

test('budget uses UTF-8 bytes, matching the controller admission contract',async t=>{
 const f=await fixture(t);f.input.goal.objective='界'.repeat(2700);
 await assert.rejects(buildResearchContext(f.input),/Required goal/);
});
test('latest acceptance outside recent failures remains validated and prevents stale base reuse',async t=>{
 const f=await fixture(t);const accepted=await f.cycle(1,f.first);
 for(let n=2;n<=5;n++)await f.cycle(n,f.second,false);
 f.input.state.base=f.second;const context=await buildResearchContext(f.input);
 assert.equal(context.provenance.sources.length,4);assert.equal(context.provenance.sources.at(-1).sequence,1);
 accepted.status.active_verification_receipt.verification_id='forged';
 await assert.rejects(buildResearchContext(f.input),/acceptance\/delivery mismatch/);
});

test('integrated receipt may verify a merge commit while preserving original candidate identity',async t=>{
 const f=await fixture(t);const c=await f.cycle(1,f.second);c.status.candidate_commit=f.first;
 const record={...c.record,candidate:f.first,acceptance:structuredClone(c.status)};
 await writeFile(join(c.dir,'result.json'),JSON.stringify(record));f.input.state.base=f.second;
 assert.match((await buildResearchContext(f.input)).text,/independently accepted software milestone/);
});

test('stale sequence or duplicate submission cannot overwrite persisted cycle evidence',async t=>{
 const f=await fixture(t);const c=await f.cycle(7,f.second);const saved=await readFile(join(c.dir,'result.json'),'utf8');
 await assert.rejects(createCycleDirectory(f.root,7),/overlaps persisted history/);
 await assert.rejects(createCycleDirectory(f.root,2),/overlaps persisted history/);
 assert.equal(await readFile(join(c.dir,'result.json'),'utf8'),saved);
 const dir=await createCycleDirectory(f.root,8);assert.equal(dir,join(f.root,'cycles','000008'));
 await assert.rejects(createCycleDirectory(f.root,8),/overlaps persisted history/);
});

test('cycle directory width is not a research iteration cap',async t=>{
 const f=await fixture(t);assert.equal(await createCycleDirectory(f.root,1000000),join(f.root,'cycles','1000000'));
 await assert.rejects(createCycleDirectory(f.root,999999),/overlaps persisted history/);
});

test('owner verifier namespace facts survive admission with provenance and do not grant acceptance',async t=>{
 const f=await fixture(t);f.input.cfg.data=f.root;await writeFile(join(f.root,'VERIFIER-CONTRACT.md'),'Inputs are mounted at /inputs; use standard temporary directories.');
 const c=await buildResearchContext(f.input);assert.match(c.text,/VERIFIER NAMESPACE FACTS/);assert.match(c.text,/not an acceptance receipt or permission expansion/);assert.equal(c.provenance.verification_contract_sha256.length,64);
});

test('approved product commit subject excludes cycle bookkeeping and validates controls',()=>{
 assert.equal(commitSubject({commit_subject:'Add model-free baselines'},{id:'internal-goal'},3),'Add model-free baselines');
 assert.equal(commitSubject({},{id:'research'},3),'research research cycle 3 (independent verification required)');
 for(const title of ['',42,'a\nb','a'.repeat(181)])assert.throws(()=>commitSubject({commit_subject:title},{id:'research'},3),/Invalid approved/);
});

async function evaluation(f,overrides={}){
 const stdout=join(f.root,'goal.stdout'),stderr=join(f.root,'goal.stderr');
 await writeFile(stdout,'');await writeFile(stderr,'AssertionError: Unicode normalization still missing');
 const receipt={commit:f.first,check_id:'user-visible-behavior',exit_status:1,stdout,stderr,
  stdout_sha256:sha(await readFile(stdout)),stderr_sha256:sha(await readFile(stderr))};
 const record={version:1,goal_id:f.input.goal.id,commit:f.first,result:{passed:false,status:'acceptance_failed',commit:f.first,phase:'check_evidence',reason:'command failed',receipts:[receipt]},...overrides};
 await writeFile(join(f.root,'goal-evaluation.json'),JSON.stringify(record));return {record,receipt};
}
async function failedCandidate(t){
 // Approved initial baseline was evaluated; a repair cycle then failed and its
 // candidate SHA advanced state.base without any accepted milestone.
 const f=await fixture(t);f.input.cfg.initial_base=f.first;await evaluation(f);await f.cycle(1,f.second,false);f.input.state.base=f.second;return f;
}
test('admission preserves pinned instructions and receipt identities when feedback excerpts exceed budget',async t=>{
 const f=await fixture(t);const task='Preserve these approved instructions: '+'X'.repeat(4200);await writeFile(join(f.root,'task.txt'),task);
 const {record,receipt}=await evaluation(f);await writeFile(receipt.stderr,'S'.repeat(15000));receipt.stderr_sha256=sha(await readFile(receipt.stderr));record.result.receipts=[receipt,{...receipt,check_id:'second-component'}];await writeFile(join(f.root,'goal-evaluation.json'),JSON.stringify(record));
 const c=await buildResearchContext(f.input);assert.ok(Buffer.byteLength(c.text)<=7900);assert.ok(c.text.includes(task));assert.match(c.text,/log_excerpts_omitted_for_budget/);assert.match(c.text,/second-component/);assert.equal(c.provenance.goal_evaluation_sha256,sha(await readFile(join(f.root,'goal-evaluation.json'))));
});
test('failed candidate retains approved baseline feedback without claiming acceptance',async t=>{
 const f=await failedCandidate(t);const context=await buildResearchContext(f.input);
 assert.match(context.text,new RegExp('EXACT REPOSITORY BASE: '+f.second));assert.match(context.text,/UNACCEPTED input/);
 assert.match(context.text,new RegExp('"commit":"'+f.first+'"'));assert.match(context.text,/Unicode normalization still missing/);
 assert.equal(context.provenance.goal_evaluation_sha256.length,64);
 const restarted=await buildResearchContext({...f.input,state:{base:f.second}});assert.equal(restarted.text,context.text);
});
test('baseline fallback rejects a foreign goal evaluation',async t=>{
 const f=await failedCandidate(t);await evaluation(f,{goal_id:'foreign-goal'});
 await assert.rejects(buildResearchContext(f.input),/identity\/result mismatch/);
});
test('baseline diagnostics stay digest checked after a failed candidate',async t=>{
 const f=await failedCandidate(t);const {receipt}=await evaluation(f);await writeFile(receipt.stderr,'forged');
 await assert.rejects(buildResearchContext(f.input),/log changed/);
});
test('baseline fallback cannot supersede a later accepted milestone',async t=>{
 const f=await fixture(t);f.input.cfg.initial_base=f.first;await evaluation(f);await f.cycle(1,f.second,true);f.input.state.base=f.second;
 await assert.rejects(buildResearchContext(f.input),/identity\/result mismatch/);
});
test('baseline fallback preserves receipt-commit validation',async t=>{
 const f=await failedCandidate(t);const {record}=await evaluation(f);record.result.receipts[0].commit=f.second;
 await writeFile(join(f.root,'goal-evaluation.json'),JSON.stringify(record));
 await assert.rejects(buildResearchContext(f.input),/receipt commit mismatch/);
});
test('independent goal failure reaches the next task after restart, distinct from passing cycle tests',async t=>{
 const f=await fixture(t);await f.cycle(1,f.first);await evaluation(f);
 const a=await buildResearchContext(f.input),b=await buildResearchContext({...f.input,state:{base:f.first}});
 assert.equal(a.text,b.text);assert.match(a.text,/INDEPENDENT WHOLE-GOAL/);assert.match(a.text,/Unicode normalization still missing/);
 assert.match(a.text,/untrusted observations/);assert.match(a.text,/"outcome":"passed"/);
 assert.equal(a.provenance.goal_evaluation_sha256.length,64);
});
test('cross-goal and stale goal evaluations cannot misdirect a later task',async t=>{
 const f=await fixture(t);await evaluation(f,{goal_id:'foreign-goal'});
 await assert.rejects(buildResearchContext(f.input),/identity\/result mismatch/);
 await evaluation(f,{commit:f.second});await assert.rejects(buildResearchContext(f.input),/identity\/result mismatch/);
});
test('goal failure logs are digest checked, including stderr',async t=>{
 const f=await fixture(t);const {receipt}=await evaluation(f);await writeFile(receipt.stderr,'forged repair instructions');
 await assert.rejects(buildResearchContext(f.input),/log changed/);
});
test('unhashed legacy stderr is never forwarded as independent diagnostics',async t=>{
 const f=await fixture(t);const {record}=await evaluation(f);delete record.result.receipts[0].stderr_sha256;
 await writeFile(join(f.root,'goal-evaluation.json'),JSON.stringify(record));
 assert.doesNotMatch((await buildResearchContext(f.input)).text,/Unicode normalization/);
});
test('large diagnostic sets remain bounded and omissions explicit after all digests are checked',async t=>{
 const f=await fixture(t);const {record,receipt}=await evaluation(f);record.result.receipts=Array.from({length:80},()=>receipt);
 await writeFile(join(f.root,'goal-evaluation.json'),JSON.stringify(record));
 const e=await loadGoalEvaluation(f.root,f.input.goal,f.first);assert(e.summary.omitted_checks>0);
 assert(Buffer.byteLength(JSON.stringify(e.summary))<3400);assert(Buffer.byteLength((await buildResearchContext(f.input)).text)<=7900);
});

// A real native loss is reconciled by scripts/reconcile-stopped-v2.mjs, which keeps
// the original failed result.json and writes a sibling reconciled-result.json that
// clears the fence. Admission must honor that independently verified evidence
// without rewriting the original and without treating the retained candidate as
// accepted.
async function reconciled(t){
 const f=await fixture(t),c=await f.cycle(1,f.second,false);f.input.state.base=f.second;f.input.cfg.controller=join(f.root,'controller-binary');await writeFile(f.input.cfg.controller,'fixture controller bytes');
 const original=join(c.dir,'result.json');
 Object.assign(c.record,{goal_id:f.input.goal.id,attempt:'attempt-1',child:'native-1',parent:'parent-1',worktree:f.repo,fence_error:'transport lost',recovery:{execution_stop:{status:'unconfirmed'},accounting:{status:'unknown'}}});
 const bytes=Buffer.from(JSON.stringify(c.record));await writeFile(original,bytes);
 const recovery={stop_id:'stop-1',attempt_id:'attempt-1',job_id:c.record.job_id,session_id:'native-1',execution_stop:{status:'confirmed'},accounting:{status:'settled_zero'}};c.status.execution_recovery=structuredClone(recovery);
 const record={...c.record,prior_fence_error:c.record.fence_error,fence_error:null,execution_stop:'confirmed',recovery,cleanup_confirmed:true,reconciliation:{original_result:original,original_result_sha256:sha(bytes),controller_sha256:sha(await readFile(f.input.cfg.controller)),method:'fixture only'}};
 const path=join(c.dir,'reconciled-result.json');await writeFile(path,JSON.stringify(record));return {...f,c,record,path,original,bytes};
}
test('authoritative reconciliation supersedes original fence without rewriting evidence or accepting the candidate',async t=>{
 const f=await reconciled(t);const ctx=await buildResearchContext(f.input);
 assert.match(ctx.text,/UNACCEPTED input/);assert.match(ctx.text,/"stop":"confirmed"/);
 assert.equal(await readFile(f.original,'utf8'),f.bytes.toString());
 assert.equal(ctx.provenance.sources[0].result_sha256,sha(await readFile(f.path)));
 assert.equal((await buildResearchContext({...f.input,state:{base:f.second,last_cycle:{sequence:0}}})).text,ctx.text);
});
test('changed original bytes invalidate reconciliation',async t=>{
 const f=await reconciled(t);await writeFile(f.original,Buffer.concat([f.bytes,Buffer.from('\n')]));await assert.rejects(buildResearchContext(f.input));
});
test('incorrect original hash and arbitrary source path are refused',async t=>{
 const f=await reconciled(t);for(const change of [{original_result_sha256:'0'.repeat(64)},{original_result:join(f.root,'unrelated.json')}]){const r={...f.record,reconciliation:{...f.record.reconciliation,...change}};await writeFile(f.path,JSON.stringify(r));await assert.rejects(buildResearchContext(f.input));}
});
test('reconciliation cannot change native identity candidate or goal',async t=>{
 const f=await reconciled(t);for(const change of [{candidate:f.first},{attempt:'other'},{child:'other'},{job_id:'other'},{goal_id:'foreign'},{worktree:'/foreign'}]){await writeFile(f.path,JSON.stringify({...f.record,...change}));await assert.rejects(buildResearchContext(f.input));}
});
test('current stop and accounting remain authoritative',async t=>{
 const f=await reconciled(t);for(const change of [{stop_id:'other'},{execution_stop:{status:'unconfirmed'}},{accounting:{status:'violating'}}]){f.c.status.execution_recovery={...f.record.recovery,...change};await assert.rejects(buildResearchContext(f.input));}
});
test('changed controller bytes refuse old reconciliation attestation',async t=>{
 const f=await reconciled(t);await writeFile(f.input.cfg.controller,'changed executable');await assert.rejects(buildResearchContext(f.input));
});
test('reconciliation cannot claim acceptance or uncertain process cleanup',async t=>{
 const f=await reconciled(t);for(const change of [{status:'accepted'},{cleanup_confirmed:false}]){await writeFile(f.path,JSON.stringify({...f.record,...change}));await assert.rejects(buildResearchContext(f.input));}
});
test('missing authoritative observation never permits replacement',async t=>{
 const f=await reconciled(t);f.input.inspect=()=>{throw Error('controller unavailable')};await assert.rejects(buildResearchContext(f.input),/controller unavailable/);
});
