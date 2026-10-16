// Admission context is rebuilt from immutable Git and independently inspected Factory records.
// Worker prose and legacy state.continuation are never acceptance evidence or cached instructions.
import {readFile,readdir} from 'node:fs/promises';
import {resolve} from 'node:path';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {recoveryAction,replacementAllowed} from './free-model-policy.mjs';
const digest=value=>createHash('sha256').update(value).digest('hex');
const oid=/^[a-f0-9]{40}$/;
const git=(repo,...args)=>execFileSync('/usr/bin/git',['-c','core.hooksPath=/dev/null','-C',repo,...args],{encoding:'utf8',timeout:30000,maxBuffer:5e6}).trim();
export function inspectCycle(controller,config,job){
 return JSON.parse(execFileSync(controller,['inspect','--config',config,'--job',job],{encoding:'utf8',timeout:40000,maxBuffer:5e6}));
}
function receiptSummary(status){
 const receipt=status.active_verification_receipt;
 return {state:status.state,revision:status.state_revision,candidate:status.candidate_commit??null,
  integrated:status.integrated_commit??null,receipt:receipt?.verification_id??null,outcome:receipt?.outcome??'none',
  checks:(receipt?.environment?.checks??[]).map(c=>({id:c.check_id,verified:c.verified,exit:c.exit_status,
   executed:c.observations?.executed??null,failed:c.observations?.failed??null})),
  stop:status.execution_recovery?.execution_stop?.status??null,accounting:status.execution_recovery?.accounting?.status??null};
}
export async function buildResearchContext({root,repo,state,goal,cfg,beforeSequence=Infinity,inspect=inspectCycle,maxBytes=7900}){
 const base=state.base;
 if(!oid.test(base??'')||git(repo,'rev-parse',base+'^{commit}')!==base)throw Error('Context base must be an exact existing commit');
 let directories=[];try{directories=await readdir(resolve(root,'cycles'))}catch(e){if(e.code!=='ENOENT')throw e}
 // Discover completed attempts afresh: last_cycle and continuation can lag a crash or handoff.
 const completed=[];
 for(const directory of directories.filter(d=>/^\d{6}$/.test(d)&&Number(d)<beforeSequence).sort().reverse()){
  const dir=resolve(root,'cycles',directory);let bytes;
  try{bytes=await readFile(resolve(dir,'result.json'))}catch(e){if(e.code==='ENOENT')continue;throw e}
  const record=JSON.parse(bytes);
  if(record.sequence!==Number(directory))throw Error('Cycle sequence identity mismatch');
  if(!record.ended_utc)throw Error('Prior cycle has no terminal evidence: '+directory);
  completed.push({dir,record,result_sha256:digest(bytes)});
 }
 const latest=completed[0]?.record;
 if(latest&&latest.status!=='all_free_unavailable'&&!(latest.status==='accepted'?recoveryAction(latest)==='continue':replacementAllowed(latest)))throw Error('Latest cycle lacks confirmed ownership/cleanup for admission');
 const latestAccepted=completed.find(c=>c.record.status==='accepted');
 const selected=completed.slice(0,3);
 if(latestAccepted&&!selected.includes(latestAccepted))selected.push(latestAccepted);
 const outcomes=[];const sources=[];let acceptedCommit=null;
 for(const {dir,record,result_sha256} of selected){
  if(!record.job_id){
   if(record.status==='accepted'||record.candidate)throw Error('Missing controller job for prior candidate');
   outcomes.push({sequence:record.sequence,state:record.status,receipt:'none',failure_kind:record.failure_kind??null});
   sources.push({sequence:record.sequence,result_sha256});continue;
  }
  const status=await inspect(cfg.controller,resolve(dir,'controller.json'),record.job_id);
  const receipt=status.active_verification_receipt;
  if(receipt&&(!oid.test(receipt.candidate_commit??'')||receipt.candidate_commit!==(status.state==='done'?status.integrated_commit:status.candidate_commit)||
    (record.candidate&&record.candidate!==status.candidate_commit)))throw Error('Prior receipt candidate mismatch');
  if(record.status==='accepted'){
   const saved=record.acceptance;
   if(status.state!=='done'||receipt?.outcome!=='passed'||!saved||status.integrated_commit!==saved.integrated_commit||
     receipt.verification_id!==saved.active_verification_receipt?.verification_id||receipt.candidate_commit!==status.integrated_commit||
     !status.delivery?.destination||status.delivery.sha256!==saved.delivery?.sha256||
     digest(await readFile(status.delivery.destination))!==status.delivery.sha256)throw Error('Prior acceptance/delivery mismatch');
   if(!acceptedCommit)acceptedCommit=status.integrated_commit;
  }
  if(record.status!=='accepted'&&status.state==='done')throw Error('Unreconciled accepted cycle; recover host handoff before admission');
  outcomes.push({sequence:record.sequence,...receiptSummary(status)});
  sources.push({sequence:record.sequence,result_sha256,revision:status.state_revision,receipt:receipt?.verification_id??null});
 }
 if(acceptedCommit){
  try{git(repo,'merge-base','--is-ancestor',acceptedCommit,base)}catch{throw Error('Selected base predates or diverges from latest accepted milestone')}
 }
 // References are taken from the exact commit, never mutable worktree files or Markdown tail positions.
 const planning=git(repo,'ls-tree','-r',base,'--','docs').split('\n').filter(Boolean)
  .map(line=>{const m=/^\d+ blob ([a-f0-9]{40})\t(.+)$/.exec(line);return m?{path:m[2],blob:m[1]}:null})
  .filter(p=>p&&/continuation|blocked|recovery|plan|research/i.test(p.path));
 const changes=git(repo,'diff-tree','--no-commit-id','--name-only','-r',base).split('\n').filter(Boolean);
 const required=[
  'STANDING GOAL (unresolved; accepted software milestones do not establish scientific completion):\n'+JSON.stringify(goal),
  'EXACT REPOSITORY BASE: '+base+'\nBASE STATUS: '+(base===acceptedCommit?'independently accepted software milestone':'UNACCEPTED input; repair and independently verify before acceptance'),
  'LATEST FACTORY OUTCOMES (newest first; structured checks are software/data observations, not scientific proof):\n'+JSON.stringify(outcomes),
  'NEXT: Inspect the repository at this exact base and the current applicable planning documents. Choose a useful experiment, repair or negative control toward the unresolved goal using accumulated evidence. Preserve frozen tests and exact evidence. Worker planning prose is unverified; reconcile its claims with these outcomes. Do not repeat an obsolete cycle directive or a cosmetic documentation patch. Report actionable missing inputs when further progress is blocked.'
 ].join('\n\n');
 if(Buffer.byteLength(required)>maxBytes)throw Error('Required goal and outcome context exceeds admission budget; no context was truncated');
 const task=await readFile(resolve(root,'task.txt'),'utf8');
 const initial=completed.length===0&&cfg.initial_continuation?'\nInitial operator guidance: '+cfg.initial_continuation:'';
 const optional='\n\nPROJECT INSTRUCTIONS:\n'+task+initial+'\n\nChanged paths at base: '+JSON.stringify(changes)+'\nPlanning references at base (path and Git blob; read applicable current entries): '+JSON.stringify(planning);
 // Instructions must also survive intact. Oversized auxiliary paths are omitted explicitly.
 const instructions='\n\nPROJECT INSTRUCTIONS:\n'+task+initial;
 if(Buffer.byteLength(required+instructions)>maxBytes)throw Error('Project instructions exceed admission budget; no instructions were truncated');
 const text=required+(Buffer.byteLength(required+optional)<=maxBytes?optional:instructions+'\nAuxiliary path inventory omitted for budget; inspect git show --stat and docs at the exact base.');
 if(Buffer.byteLength(text)>maxBytes)throw Error('Context exceeds admission budget');
 return {text,provenance:{version:1,base,goal_sha256:digest(JSON.stringify(goal)),sources,context_sha256:digest(text),legacy_continuation_ignored:true}};
}
