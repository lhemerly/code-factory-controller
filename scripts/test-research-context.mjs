import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {buildResearchContext,createCycleDirectory,commitSubject} from './research-context.mjs';
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
