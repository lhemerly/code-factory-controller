import test from 'node:test';
import assert from 'node:assert/strict';
import {awaitNativeCompletion} from './native-completion.mjs';
const sid='owned',worktree='/work';
function fixture(){
  const f={clock:0,polls:0,renewals:0,active:true,inbox:[],shells:[],revision:1};
  f.options={sessionID:sid,worktree,lease:{tick:async()=>f.renewals++,check:()=>{}},
    getSession:async()=>({id:sid,location:{directory:worktree},time:{updated:f.revision},outcome:'succeeded'}),
    getActive:async()=>f.active?{[sid]:{type:'running'}}:{},getInbox:async()=>f.inbox,
    getShells:async()=>({location:{directory:worktree},data:f.shells}),
    checkpoint:async()=>{},resources:async()=>{},shouldStop:()=>false,now:()=>f.clock,
    sleep:async()=>{f.clock+=2000;if(++f.polls===3){f.active=false;f.revision=2}}};
  return f;
}
test('cached success plus drained shells waits for resumed execution and its final source revision',async()=>{
  const f=fixture();const result=await awaitNativeCompletion(f.options);
  assert.equal(result.time.updated,2);assert.equal(f.polls,4);assert.equal(f.renewals,5);
});
test('late inbox and shell completion reset quiet confirmation while ownership renews',async()=>{
  const f=fixture();f.active=false;
  f.options.sleep=async()=>{f.clock+=3600000;f.polls++;
    f.inbox=f.polls<4?[{id:'completion'}]:[];
    f.shells=f.polls<4?[{id:'shell',cwd:worktree,status:'running',time:{started:1}}]:[];
    f.revision=f.polls<4?1:2;
  };
  const result=await awaitNativeCompletion(f.options);
  assert.equal(result.time.updated,2);assert.equal(f.polls,5);assert.equal(f.renewals,6);
});
test('lost lease cannot be resurrected by a terminal outcome',async()=>{
  const f=fixture();let reads=0;f.options.lease.tick=async()=>{throw Error('stale lease')};
  f.options.getSession=async()=>{reads++;return{}};
  await assert.rejects(awaitNativeCompletion(f.options),/stale lease/);assert.equal(reads,0);
});
test('foreign or malformed completion observations fail closed',async()=>{
  for(const change of [f=>f.options.getSession=async()=>({id:'foreign'}),f=>f.options.getActive=async()=>[],f=>f.options.getInbox=async()=>({data:[]}),f=>f.shells=[{id:'s',cwd:'/foreign',status:'exited',time:{completed:1}}]]){
    const f=fixture();change(f);await assert.rejects(awaitNativeCompletion(f.options),/unknown/);
  }
});
test('explicit owner stop and resource failure remain authoritative',async()=>{
  const f=fixture();f.options.shouldStop=()=>true;
  await assert.rejects(awaitNativeCompletion(f.options),/Operator stopping/);
  const g=fixture();g.options.resources=async()=>{throw Error('Disk resource limit')};
  await assert.rejects(awaitNativeCompletion(g.options),/Disk resource/);
});
