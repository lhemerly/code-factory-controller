// Completion is an owned native observation, never the session's cached outcome alone.
import {runningShells} from './continuous-ownership.mjs';
export async function awaitNativeCompletion({sessionID,worktree,getSession,getActive,getInbox,getShells,checkpoint,resources,lease,shouldStop,sleep,now=Date.now}) {
  let previous, saved=-Infinity, checked=-Infinity;
  while(!shouldStop()) {
    await lease.tick();lease.check();
    const session=await getSession();
    if(session.id!==sessionID||session.location?.directory!==worktree||!Number.isFinite(session.time?.updated))throw Error('Native completion ownership/update unknown');
    const active=await getActive(),inbox=await getInbox(),shells=await getShells();
    if(!active||typeof active!=='object'||Array.isArray(active)||!Array.isArray(inbox)||inbox.length>4096)throw Error('Native completion activity/inbox unknown');
    const quiet=!Object.hasOwn(active,sessionID)&&inbox.length===0&&runningShells(shells,worktree).length===0;
    const observation={session,active:Object.hasOwn(active,sessionID),inbox_count:inbox.length,shells,quiet};
    if(now()-saved>=15000){await checkpoint(observation);saved=now()}
    if(now()-checked>=60000){await resources();checked=now()}
    if(quiet&&previous===session.time.updated&&session.outcome) return session;
    previous=quiet?session.time.updated:undefined;
    await sleep(2000);
  }
  throw Error('Operator stopping');
}
