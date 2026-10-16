// Actual pinned OpenCode shell API regression. No model/provider calls.
import {spawn,execFileSync} from 'node:child_process';
import {mkdtemp,mkdir,writeFile,readFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';import {resolve} from 'node:path';import {createServer} from 'node:net';import assert from 'node:assert/strict';
import {OpenCode} from './opencode-v2-client.mjs';
import {awaitNativeShells,OwnershipLease,runningShells} from './continuous-ownership.mjs';
const root=await mkdtemp(resolve(tmpdir(),'native-shell-contract-')),repo=resolve(root,'repository'),profile=resolve(root,'profile');await mkdir(repo);await mkdir(profile);
execFileSync('/usr/bin/git',['init','-q',repo]);await writeFile(resolve(repo,'opencode.json'),JSON.stringify({plugins:[],update:'disable',share:'disabled',snapshots:false}));
for(const name of ['home','config','data','cache','state'])await mkdir(resolve(profile,name));
const port=await new Promise(ok=>{const s=createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>ok(p))})});
const native=resolve(process.argv[2]??'../runtime/node_modules/@opencode/cli-linux-x64/bin/opencode');
const p=spawn(native,['serve','--hostname','127.0.0.1','--port',String(port)],{cwd:repo,env:{PATH:'/usr/bin:/bin',HOME:resolve(profile,'home'),XDG_CONFIG_HOME:resolve(profile,'config'),XDG_DATA_HOME:resolve(profile,'data'),XDG_CACHE_HOME:resolve(profile,'cache'),XDG_STATE_HOME:resolve(profile,'state')},stdio:['ignore','pipe','pipe']});let output='';for(const stream of [p.stdout,p.stderr])stream.on('data',b=>output+=b.toString());
const sleep=ms=>new Promise(ok=>setTimeout(ok,ms));
try{
 let password;for(let i=0;i<150;i++){password=/server password (\S+)/.exec(output)?.[1];if(password&&output.includes('server listening on'))break;if(p.exitCode!==null)throw Error('Native startup failed');await sleep(100)}assert(password);
 const client=OpenCode.make({baseUrl:'http://127.0.0.1:'+port,headers:{authorization:'Basic '+Buffer.from('opencode:'+password).toString('base64'),'x-opencode-directory':repo}});assert.equal((await client.server.info()).version,'2.0.29');
 const location={directory:repo};const created=await client.shell.create({location,cwd:repo,command:'sleep 2; printf native-background-complete',timeout:10000});assert.equal(created.location.directory,repo);assert.equal(created.data.status,'running');let observations=0,polls=0;assert((await client.shell.list({location})).data.some(s=>s.id===created.data.id&&s.status==='running'));
 const quiet=await awaitNativeShells({get:()=>{polls++;return client.shell.list({location})},worktree:repo,checkpoint:async()=>observations++,resources:async()=>{},lease:new OwnershipLease(async()=>{}),shouldStop:()=>false,sleep:async()=>sleep(50)});
 assert.equal(runningShells(quiet,repo).length,0);assert(polls>1); const terminal=await client.shell.get({location,id:created.data.id});assert.equal(terminal.data.status,'exited');assert(Number.isFinite(terminal.data.time.completed));
 const active=await client.shell.create({location,cwd:repo,command:'sleep 300',timeout:0});assert.equal(active.data.status,'running');assert(Number.isInteger(active.data.pid)&&active.data.pid>1,'Native shell PID missing');await client.shell.remove({location,id:active.data.id});const stopped=await client.shell.list({location});assert(!stopped.data.some(s=>s.id===active.data.id&&s.status==='running'));
 // Native shell deletion must stop its actual owned process, not just hide a row.
 if(active.data.pid){let alive=true;for(let i=0;i<50;i++){try{process.kill(active.data.pid,0)}catch(e){if(e.code==='ESRCH'){alive=false;break}throw e}await sleep(20)}assert.equal(alive,false,'Deleted shell OS process still alive')}
 console.log(JSON.stringify({passed:true,runtime:'2.0.29',provider_calls:0,terminal_shell_history_count:quiet.data.length,background_wait_polls:polls,background_wait_observations:observations,owned_shell_delete_and_os_exit:true,profile:root},null,2));
}finally{p.kill('SIGTERM');for(let i=0;i<50&&p.exitCode===null&&p.signalCode===null;i++)await sleep(100);if(p.exitCode===null&&p.signalCode===null)p.kill('SIGKILL')}
