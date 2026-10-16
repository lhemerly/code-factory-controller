import test from 'node:test'; import assert from 'node:assert/strict';
import {freeModels,classify,route,failure,recoveryAction,replacementAllowed,approvedScope,researchContinuation} from './free-model-policy.mjs';
const m=id=>({id,providerID:'opencode',enabled:true,status:'active',cost:[{input:0,output:0,cache:{read:0,write:0}}]});
test('fresh catalog rejects unknown/paid/disabled prices and prefers proven Big Pickle',()=>{
 const bad=m('paid');bad.cost.push({input:1,output:0,cache:{read:0,write:0}});
 assert.deepEqual(freeModels([m('z-free'),bad,m('big-pickle'),{...m('off'),enabled:false},{...m('unknown'),cost:[]}]).map(m=>m.id),['opencode/big-pickle','opencode/z-free']);
});
test('injected denial persists, routes next exact free ID and never probes denial again',()=>{
 const models=freeModels([m('big-pickle'),m('b-free')]);const c=failure({},models[0].id,classify('HTTP403 FreeTierError'),100);
 assert.equal(route(models,c,100),models[1].id);assert.equal(route(models,c,Date.now()+86400000),models[1].id);
 assert.equal(classify('test failed','verification'),'repair');assert.equal(classify('HTTP429 quota'),'unavailable');
 assert.equal(classify('Factory model has nonzero or unknown pricing'),'unavailable');
 assert.equal(classify('getaddrinfo ETIMEOUT opencode.ai'),'transient');
});
test('transient exponential backoff and all-free-unavailable pause without paid fallback',()=>{
 const models=freeModels([m('big-pickle')]);let c=failure({},models[0].id,'transient',0);assert.equal(route(models,c,1),undefined);
 assert.equal(route(models,c,60000),models[0].id);c=failure(c,models[0].id,'transient',60000);assert.equal(c[models[0].id].until,180000);
});
test('restart replacement requires positive ownership/cleanup evidence',()=>{
 assert.equal(recoveryAction({status:'accepted',cleanup_confirmed:false}),'pause');
 assert.equal(recoveryAction({status:'failed',cleanup_confirmed:true,execution_stop:'unknown'}),'pause');
 assert.equal(recoveryAction({status:'failed',cleanup_confirmed:true,execution_stop:'confirmed'}),'continue');
 assert.equal(recoveryAction({status:'accepted',cleanup_confirmed:true}),'continue');
});

test('fresh-job failover preserves unknown accounting but refuses violating spend or uncertain stop',()=>{
 const failed={cleanup_confirmed:true,recovery:{execution_stop:{status:'confirmed'},accounting:{status:'unknown',observations:[{tokens:null}]}}};
 assert.equal(replacementAllowed(failed),true);assert.equal(failed.recovery.accounting.observations[0].tokens,null);
 assert.equal(replacementAllowed({...failed,recovery:{...failed.recovery,accounting:{status:'violating'}}}),false);
 assert.equal(replacementAllowed({...failed,recovery:{...failed.recovery,execution_stop:{status:'unconfirmed'}}}),false);
 assert.equal(replacementAllowed({...failed,fence_error:'interrupt failed'}),false);
});

test('unaccepted repair bases cannot absorb forbidden changes into a new approved scope',()=>{
 const template={allowed_paths:['spark_tracker','tests','docs'],protected_paths:['tests/frozen.py']};
 assert.equal(approvedScope(['spark_tracker/lamina.py','tests/new_regression.py','docs/results.md'],template),true);
 assert.equal(approvedScope(['tests/frozen.py'],template),false);
 assert.equal(approvedScope(['tests-escape/new.py'],template),false);
 assert.equal(approvedScope(['.gitignore'],template),false);
});

test('inconclusive science, worker blocker prose and rejected candidates request useful continuation',()=>{
 for(const out of [{status:'accepted',scientific_blocker:'No stronger labels in current region'},{status:'failed',error:'No approved candidate changes'},{status:'failed',error:'Independent verification failed'},{status:'accepted',continuation:'Try an independent negative control'}]) {
  const next=researchContinuation(out);assert.match(next,/Continue useful research/);assert.match(next,/negative control/);assert.match(next,/not repeat a cosmetic/);
 }
 assert.match(researchContinuation({status:'failed'}),/unaccepted repair input/);
});

test('genuine permission and ownership denial never become scientific repair or provider fallback',()=>{for(const error of ['PermissionDeniedError','permission denied','requires approval','scope violation','ownership changed','V2 active or unknown shell work'])assert.equal(classify(error,'verification'),'blocked')});
