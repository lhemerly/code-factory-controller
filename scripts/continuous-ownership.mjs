// Ownership liveness is renewable and independent of research elapsed duration.
export class OwnershipLease {
  constructor(renew, {intervalMs=60000, now=Date.now}={}) {
    this.renew=renew; this.intervalMs=intervalMs; this.now=now;
    this.next=0; this.pending=undefined; this.error=undefined;
  }
  async tick(force=false) {
    this.check();
    if(this.pending)return this.pending;
    if(!force&&this.now()<this.next)return;
    this.pending=Promise.resolve().then(()=>this.renew()).then(()=>{this.next=this.now()+this.intervalMs},error=>{this.error=error;throw error}).finally(()=>{this.pending=undefined});
    return this.pending;
  }
  check(){if(this.error)throw Error('Ownership lease renewal failed; stop and reconcile: '+this.error)}
  start(){this.timer=setInterval(()=>{this.tick().catch(()=>{})},this.intervalMs);return this}
  async stop(){clearInterval(this.timer);if(this.pending)await this.pending.catch(()=>{})}
}
export class RepeatedToolFailure extends Error {
  constructor(observation){super('Repeated native tool failures; inspect tool arguments and approved worktree scope before a fresh attempt');this.observation=observation;}
}
export function toolFailureObservation(context) {
  if(!Array.isArray(context))throw Error('Native tool history observation unknown');
  const results=[];
  for(const message of context){
    if(message.type!=='assistant'||!Array.isArray(message.content))continue;
    for(const part of message.content){
      if(part.type!=='tool'||!['completed','error'].includes(part.state?.status))continue;
      if(part.state.status==='completed'&&['edit','write','apply_patch'].includes(part.name)){results.length=0;continue;}
      const kind=String(part.state.error?.type??'unknown').slice(0,80);
      results.push({tool:String(part.name??'unknown').slice(0,80),failed:part.state.status==='error',kind,
        denied:/permission|authorization|policy/.test(kind)||/permission.?denied|requires?.?approval|forbidden|unauthori[sz]ed/i.test(part.state.error?.message??''),
        unknown:!['tool.execution','tool.validation'].includes(kind)});
    }
  }
  const recent=results.slice(-6),errors=recent.filter(r=>r.failed);
  // Tool protocol/access errors, not a shell's nonzero test exit, trigger a
  // fresh attempt. No elapsed work limit or worker-authored blocker is used.
  return errors.length>=4||errors.some(r=>r.denied)?{recent_results:recent.length,error_count:errors.length,tools:[...new Set(errors.map(r=>r.tool))],error_kinds:[...new Set(errors.map(r=>r.kind))],permission_denied:errors.some(r=>r.denied),unknown_error:errors.some(r=>r.unknown)}:null;
}
export async function observeResearch({get,checkpoint,resources,lease,shouldStop,sleep,now=Date.now,toolContext}) {
  let saved=-Infinity,resourceCheck=-Infinity;
  while(!shouldStop()) {
    await lease.tick();lease.check();
    const session=await get();
    if(now()-saved>=15000){
      await checkpoint(session);saved=now();
      if(toolContext&&!session.outcome){
        const failure=toolFailureObservation(await toolContext());
        if(failure?.permission_denied)throw Error('Permission denied in native tool history; inspect approved worktree scope');
        if(failure?.unknown_error)throw Error('Native tool failure has unknown policy/ownership semantics');
        if(failure)throw new RepeatedToolFailure(failure);
      }
    }
    if(now()-resourceCheck>=60000){await resources();resourceCheck=now()}
    if(session.outcome)return session;
    await sleep(2000);
  }
  throw Error('Operator stopping');
}

export function runningShells(observation,worktree) {
  if(observation?.location?.directory!==worktree||!Array.isArray(observation.data)||observation.data.length>4096)throw Error('Native shell ownership/observation unknown');
  return observation.data.filter(shell=>{
    if(!shell.id||typeof shell.cwd!=='string'||!(shell.cwd===worktree||shell.cwd.startsWith(worktree+'/'))||shell.cwd.split('/').includes('..'))throw Error('Native shell scope unknown');
    if(shell.status==='running')return true;
    if(!['exited','timeout','killed'].includes(shell.status)||!Number.isFinite(shell.time?.completed))throw Error('Native shell completion unknown');
    return false;
  });
}
export async function awaitNativeShells({get,worktree,checkpoint,resources,lease,shouldStop,sleep,now=Date.now}) {
  let saved=-Infinity,checked=-Infinity;
  while(!shouldStop()) {
    await lease.tick();lease.check();const observation=await get();
    if(now()-saved>=15000){await checkpoint(observation);saved=now()}
    const running=runningShells(observation,worktree);
    if(!running.length)return observation;
    if(now()-checked>=60000){await resources();checked=now()}
    await sleep(2000); // Productive native background work has no elapsed cutoff.
  }
  throw Error('Operator stopping');
}
