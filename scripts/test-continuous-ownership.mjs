import test from 'node:test';import assert from 'node:assert/strict';
import {OwnershipLease,observeResearch} from './continuous-ownership.mjs';
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
