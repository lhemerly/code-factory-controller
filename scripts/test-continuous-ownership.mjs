import test from 'node:test';import assert from 'node:assert/strict';
import {OwnershipLease,observeResearch,RepeatedToolFailure,toolFailureObservation} from './continuous-ownership.mjs';
test('productive work crosses 20 minutes and eight hours with ownership renewed and checkpoints',async()=>{
 let clock=0,renewals=0,checkpoints=0,polls=0;const lease=new OwnershipLease(async()=>renewals++,{now:()=>clock});
 const terminal=await observeResearch({get:async()=>({time:{updated:clock},...(++polls===1001?{outcome:'succeeded'}:{})}),checkpoint:async()=>checkpoints++,resources:async()=>{},lease,shouldStop:()=>false,sleep:async()=>{clock+=60000},now:()=>clock});
 assert.equal(terminal.outcome,'succeeded');assert.equal(clock,1000*60000);assert(renewals>480);assert(checkpoints>480);
});
test('ownership failure fences execution instead of resurrecting a lost lease',async()=>{
 let calls=0;const lease=new OwnershipLease(async()=>{throw Error('stale lease')});
 await assert.rejects(observeResearch({get:async()=>{calls++;return{}},checkpoint:async()=>{},resources:async()=>{},lease,shouldStop:()=>false,sleep:async()=>{}}),/stale lease/);assert.equal(calls,0);
});
test('user stop and actual resource failure remain stop conditions',async()=>{
 const lease=new OwnershipLease(async()=>{});
 await assert.rejects(observeResearch({get:async()=>({}),checkpoint:async()=>{},resources:async()=>{},lease,shouldStop:()=>true,sleep:async()=>{}}),/Operator stopping/);
 await assert.rejects(observeResearch({get:async()=>({}),checkpoint:async()=>{},resources:async()=>{throw Error('Disk resource limit reached')},lease,shouldStop:()=>false,sleep:async()=>{}}),/Disk resource/);
});
test('pending renewal is serialized; delayed control replies cannot duplicate heartbeats',async()=>{
 let count=0,release;const lease=new OwnershipLease(()=>{count++;return new Promise(ok=>release=ok)});
 const first=lease.tick();const second=lease.tick();await new Promise(ok=>setImmediate(ok));assert.equal(count,1);release();await Promise.all([first,second]);
});

test('terminal model turn waits for owned background shells without a research deadline',async()=>{
 const {awaitNativeShells}=await import('./continuous-ownership.mjs');let clock=0,polls=0,renewals=0,saved=0;
 const lease=new OwnershipLease(async()=>renewals++,{now:()=>clock});
 await awaitNativeShells({get:async()=>({location:{directory:'/work'},data:[{id:'sh_eval',cwd:'/work',status:++polls<1001?'running':'exited',time:{completed:clock}}]}),worktree:'/work',checkpoint:async()=>saved++,resources:async()=>{},lease,shouldStop:()=>false,sleep:async()=>clock+=60000,now:()=>clock});
 assert.equal(clock,60000000);assert(renewals>480);assert(saved>480);
});
test('completed shell history is quiet but unknown, running and foreign work never qualifies',async()=>{
 const {runningShells,awaitNativeShells}=await import('./continuous-ownership.mjs');const item={id:'sh_test',cwd:'/work',status:'exited',time:{completed:4}};
 assert.deepEqual(runningShells({location:{directory:'/work'},data:[item]},'/work'),[]);
 assert.equal(runningShells({location:{directory:'/work'},data:[{...item,status:'running'}]},'/work').length,1);
 for(const bad of [{...item,status:'unknown'},{...item,time:{}},{...item,cwd:'/foreign'},{...item,cwd:'/work/../foreign'}])assert.throws(()=>runningShells({location:{directory:'/work'},data:[bad]},'/work'),/unknown/);
 await assert.rejects(awaitNativeShells({shouldStop:()=>true}),/Operator stopping/);
});
const tools=parts=>[{type:'assistant',content:parts}];
const failed=()=>({type:'tool',name:'read',state:{status:'error',error:{type:'tool.execution',message:'untrusted path and prose'}}});
test('repeated tool errors trigger owned recovery without forwarding raw error prose',async()=>{
 const context=tools(Array.from({length:4},failed));const observation=toolFailureObservation(context);
 assert.equal(observation.error_count,4);assert(!JSON.stringify(observation).includes('untrusted path'));
 const lease=new OwnershipLease(async()=>{});let saved=0;
 await assert.rejects(observeResearch({get:async()=>({}),toolContext:async()=>context,checkpoint:async()=>saved++,resources:async()=>{},lease,shouldStop:()=>false,sleep:async()=>{}}),error=>error instanceof RepeatedToolFailure&&error.observation.error_count===4);
 assert.equal(saved,1);
});
test('test failures and successful implementation are not mistaken for tool protocol failure',()=>{
 assert.equal(toolFailureObservation(tools(Array.from({length:8},()=>({type:'tool',name:'shell',state:{status:'completed',metadata:{exit:1}}})))),null);
 assert.equal(toolFailureObservation(tools([...Array.from({length:4},failed),{type:'tool',name:'edit',state:{status:'completed'}}])),null);
 assert.equal(toolFailureObservation(tools([...Array.from({length:4},failed),...Array.from({length:6},()=>({type:'tool',name:'read',state:{status:'completed'}}))])),null);
 assert.equal(toolFailureObservation(tools(Array.from({length:3},failed))),null);
});
test('permission rejection remains a blocker and never becomes model failover',async()=>{
 const context=tools([{type:'tool',name:'read',state:{status:'error',error:{type:'permission.rejected',message:'Permission denied: external_directory'}}}]);
 const lease=new OwnershipLease(async()=>{});
 await assert.rejects(observeResearch({get:async()=>({}),toolContext:async()=>context,checkpoint:async()=>{},resources:async()=>{},lease,shouldStop:()=>false,sleep:async()=>{}}),error=>!(error instanceof RepeatedToolFailure)&&/Permission denied/.test(error.message));
});

test('permission rejection survives successful edits and long subsequent tool history',()=>{
 const denial={type:'tool',name:'shell',state:{status:'error',error:{type:'permission.rejected',message:'Permission denied: external_directory'}}};
 const after=[{type:'tool',name:'edit',state:{status:'completed'}},...Array.from({length:8},()=>({type:'tool',name:'read',state:{status:'completed'}}))];
 assert.equal(toolFailureObservation(tools([denial,...after])).permission_denied,true);
});
test('permission history is checked before terminal success and between checkpoints',async()=>{
 const context=tools([{type:'tool',name:'shell',state:{status:'error',error:{type:'permission.rejected'}}}]);
 for(const terminal of [false,true]){
  let polls=0,clock=0,saved=0;const lease=new OwnershipLease(async()=>{},{now:()=>clock});
  await assert.rejects(observeResearch({get:async()=>{polls++;return terminal?{outcome:'succeeded'}:{}},toolContext:async()=>terminal||polls===2?context:[],checkpoint:async()=>saved++,resources:async()=>{},lease,shouldStop:()=>polls>3,sleep:async()=>clock+=2000,now:()=>clock}),/Permission denied/);
  assert.equal(polls,terminal?1:2);assert.equal(saved,1);
 }
});
