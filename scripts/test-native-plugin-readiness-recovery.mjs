import {test} from 'node:test';import assert from 'node:assert/strict';import {waitForFactoryPlugin} from './native-plugin-readiness.mjs';

function fixture(sequence,commands=true){
 let n=0,time=0;
 const states=Array.isArray(sequence)?sequence:[sequence];
 return{
  client:{
   command:{list:async()=>({data:commands?[{name:'pstack-factory-start'}]:[]})},
   plugin:{list:async()=>{
    const s=states[Math.min(n++,states.length-1)];
    if(typeof s==='function')return await s();
    return {data:s};
   }}
  },
  options:{timeoutMs:500,now:()=>time,sleep:async ms=>{time+=ms}}
 };
}

test('transient ECONNRESET during startup recovers within deadline',async()=>{
 const econreset=Object.assign(new Error('connection reset'),{code:'ECONNRESET'});
 const f=fixture([
  async()=>{throw econreset;},
  [],
  [{id:'pstack.opencode',state:{status:'loading'}}],
  [{id:'pstack.opencode',state:{status:'active'}}]
 ]);
 const seen=[];
 const p=await waitForFactoryPlugin(f.client,{...f.options,checkpoint:async x=>seen.push(x)});
 assert.equal(p.state.status,'active');
 assert(seen.length>=4);
});

test('transient 503 service unavailable recovers within deadline',async()=>{
 const unavailable=new Error('service unavailable');unavailable.status=503;
 const f=fixture([
  async()=>{throw unavailable;},
  [{id:'pstack.opencode',state:{status:'loading'}}],
  [{id:'pstack.opencode',state:{status:'active'}}]
 ]);
 const p=await waitForFactoryPlugin(f.client,{...f.options});
 assert.equal(p.state.status,'active');
});

test('repeated transient failures eventually time out without unlimited retries',async()=>{
 const econreset=Object.assign(new Error('connection reset'),{code:'ECONNRESET'});
 const f=fixture([async()=>{throw econreset;},async()=>{throw econreset;},async()=>{throw econreset;}]);
 await assert.rejects(waitForFactoryPlugin(f.client,{...f.options,timeoutMs:200}),/timed out/);
});

test('permission denied fails immediately without retry',async()=>{
 const perm=new Error('permission denied');perm.status=403;
 const calls=[];
 const f=fixture([async()=>{
  calls.push('call');
  throw perm;
 }]);
 await assert.rejects(waitForFactoryPlugin(f.client,{...f.options}),/permission denied/);
 assert.equal(calls.length,1);
});

test('terminal plugin error fails immediately',async()=>{
 const f=fixture([[{id:'pstack.opencode',state:{status:'error',error:'forbidden'}}]]);
 await assert.rejects(waitForFactoryPlugin(f.client,f.options),/forbidden/);
});
