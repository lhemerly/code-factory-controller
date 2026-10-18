import test from 'node:test';
import assert from 'node:assert/strict';
import {awaitNativeCompletion} from './native-completion.mjs';
import {readFile} from 'node:fs/promises';
import {runInNewContext} from 'node:vm';
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
test('completion retains the existing repeated-tool-failure recovery guard',async()=>{
  const f=fixture();f.options.toolContext=async()=>[{type:'assistant',content:Array.from({length:4},()=>({type:'tool',name:'read',state:{status:'error',error:{type:'tool.execution'}}}))}];
  await assert.rejects(awaitNativeCompletion(f.options),/Repeated native tool failures/);
});
test('actual host seal transport outlasts two bounded observations; ordinary calls retain their budget',async()=>{
  const source=await readFile(new URL('./spark-continuous.mjs',import.meta.url),'utf8');
  const definition=source.split('\n').find(line=>line.trimStart().startsWith('async function op('));assert(definition);
  // Scale seconds to milliseconds, preserving the real function's deadline
  // selection and AbortSignal behavior without a minute-long test.
  const op=runInNewContext(definition+'\nop',{controllerURL:'http://fixture',operator:'fixture',job:'fixture',dir:'/tmp',resolve:()=>'/tmp',atomic:async()=>{},AbortSignal:{timeout:ms=>AbortSignal.timeout(ms/1000)},fetch:async(_url,{signal})=>new Promise((ok,bad)=>{const timer=setTimeout(()=>ok({ok:true,json:async()=>({qualified:true})}),55);signal.addEventListener('abort',()=>{clearTimeout(timer);bad(Error('transport deadline'))},{once:true})})});
  assert.equal((await op('seal-v2')).qualified,true);
  await assert.rejects(op('status'),/transport deadline/);
});
