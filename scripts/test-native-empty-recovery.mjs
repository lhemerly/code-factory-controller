// Actual pinned V2 persistence/deny-all contract. No prompt or provider calls.
import {spawn,execFileSync} from 'node:child_process';
import {mkdtemp,mkdir,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';import {resolve} from 'node:path';import {createServer} from 'node:net';import assert from 'node:assert/strict';
import {OpenCode} from './opencode-v2-client.mjs';
import {completeSessionInventory} from './native-session-inventory.mjs';
const root=await mkdtemp(resolve(tmpdir(),'native-empty-recovery-')),repo=resolve(root,'repository'),profile=resolve(root,'profile');await mkdir(repo);await mkdir(profile);
execFileSync('/usr/bin/git',['init','-q',repo]);await writeFile(resolve(repo,'opencode.json'),JSON.stringify({plugins:[],update:'disable',share:'disabled',snapshots:false}));
for(const name of ['home','config','data','cache','state'])await mkdir(resolve(profile,name));
const native=resolve(process.argv[2]??'../runtime/node_modules/@opencode/cli-linux-x64/bin/opencode'),sleep=ms=>new Promise(ok=>setTimeout(ok,ms));
let session;
for(let pass=0;pass<2;pass++){
 const port=await new Promise(ok=>{const s=createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>ok(p))})});
 const p=spawn(native,['serve','--hostname','127.0.0.1','--port',String(port)],{cwd:repo,env:{PATH:'/usr/bin:/bin',HOME:resolve(profile,'home'),XDG_CONFIG_HOME:resolve(profile,'config'),XDG_DATA_HOME:resolve(profile,'data'),XDG_CACHE_HOME:resolve(profile,'cache'),XDG_STATE_HOME:resolve(profile,'state')},stdio:['ignore','pipe','pipe']});let output='';for(const stream of [p.stdout,p.stderr])stream.on('data',b=>output=(output+b.toString()).slice(-100000));
 try{
  let password;for(let i=0;i<150;i++){password=/server password (\S+)/.exec(output)?.[1];if(password&&output.includes('server listening on'))break;if(p.exitCode!==null)throw Error('Native startup failed');await sleep(100)}assert(password);
  const client=OpenCode.make({baseUrl:'http://127.0.0.1:'+port,headers:{authorization:'Basic '+Buffer.from('opencode:'+password).toString('base64'),'x-opencode-directory':repo},fetch:(u,o={})=>fetch(u,{...o,signal:AbortSignal.timeout(20000)})});assert.equal((await client.server.info()).version,'2.0.29');
  const list=await completeSessionInventory(client);assert.equal(list.length,pass);
  if(pass===0)session=await client.session.create({title:'Empty admission recovery contract',location:{directory:repo},metadata:{factoryRecovery:{test:true}},permissions:[{action:'*',resource:'*',effect:'deny'}]});
  const info=await client.session.get({sessionID:session.id});
  assert.equal(info.id,session.id);assert.equal(info.location.directory,repo);assert.equal(info.parentID??null,null);assert.deepEqual(info.permissions,[{action:'*',resource:'*',effect:'deny'}]);assert.equal(info.cost,0);assert.deepEqual(info.tokens,{input:0,output:0,reasoning:0,cache:{read:0,write:0}});assert.deepEqual(info.metadata.factoryRecovery,{test:true});
  const exported=await client.session.export({sessionID:session.id});assert.equal(exported.messages.length,0);
  const after=await completeSessionInventory(client);assert.equal(after.length,1);assert.equal(after[0].id,session.id);
 }finally{p.kill('SIGTERM');for(let i=0;i<50&&p.exitCode===null&&p.signalCode===null;i++)await sleep(100);if(p.exitCode===null&&p.signalCode===null){p.kill('SIGKILL');await new Promise(ok=>p.once('exit',ok))}assert(p.exitCode!==null||p.signalCode!==null)}
}
console.log(JSON.stringify({passed:true,runtime:'2.0.29',provider_calls:0,empty_deny_all_session_survives_restart:true,owned_servers_exited:true,profile:root},null,2));
