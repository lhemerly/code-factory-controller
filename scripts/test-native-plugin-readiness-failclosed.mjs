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

test('HTTP403 with service unavailable message rejects immediately without retry',async()=>{
 const err=new Error('service unavailable (503)');
 err.status=403;
 const calls=[];
 const f=fixture([async()=>{
  calls.push('call');
  throw err;
 }]);
 await assert.rejects(waitForFactoryPlugin(f.client,f.options),e=>e.message.includes('service unavailable (503)'));
 assert.equal(calls.length,1);
});

test('EACCES with reset message rejects immediately without retry',async()=>{
 const err=new Error('connection reset by peer');
 err.code='EACCES';
 const calls=[];
 const f=fixture([async()=>{
  calls.push('call');
  throw err;
 }]);
 await assert.rejects(waitForFactoryPlugin(f.client,f.options),e=>e.message.includes('connection reset by peer'));
 assert.equal(calls.length,1);
});

test('unknown path error containing 503 rejects immediately without retry',async()=>{
 const err=new Error('Cannot read configuration /config/503.json');
 const calls=[];
 const f=fixture([async()=>{
  calls.push('call');
  throw err;
 }]);
 await assert.rejects(waitForFactoryPlugin(f.client,f.options),e=>e.message.includes('Cannot read configuration /config/503.json'));
 assert.equal(calls.length,1);
});

test('terminal pstack plugin error with service unavailable rejects immediately',async()=>{
 const f=fixture([[{id:'pstack.opencode',state:{status:'error',error:'service unavailable (503)'}}]]);
 await assert.rejects(waitForFactoryPlugin(f.client,f.options),e=>e.message.includes('service unavailable (503)'));
});

test('typed HTTP503 still recovers as transient',async()=>{
 const unavailable=new Error('service unavailable');
 unavailable.status=503;
 const f=fixture([
  async()=>{throw unavailable;},
  [{id:'pstack.opencode',state:{status:'loading'}}],
  [{id:'pstack.opencode',state:{status:'active'}}]
 ]);
 const p=await waitForFactoryPlugin(f.client,f.options);
 assert.equal(p.state.status,'active');
});

test('ECONNRESET still recovers as transient',async()=>{
 const econreset=Object.assign(new Error('connection reset'),{code:'ECONNRESET'});
 const f=fixture([
  async()=>{throw econreset;},
  [],
  [{id:'pstack.opencode',state:{status:'loading'}}],
  [{id:'pstack.opencode',state:{status:'active'}}]
 ]);
 const p=await waitForFactoryPlugin(f.client,f.options);
 assert.equal(p.state.status,'active');
});
