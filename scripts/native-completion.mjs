// Completion is an owned native observation, never the session's cached outcome alone.
import {observeResearch,runningShells} from './continuous-ownership.mjs';
export async function awaitNativeCompletion({sessionID,worktree,getSession,getActive,getInbox,getShells,checkpoint,...controls}) {
  let previous,observation;
  return observeResearch({...controls,checkpoint:session=>checkpoint({...observation,session}),get:async()=>{
    const session=await getSession();
    if(session.id!==sessionID||session.location?.directory!==worktree||!Number.isFinite(session.time?.updated))throw Error('Native completion ownership/update unknown');
    const active=await getActive(),inbox=await getInbox(),shells=await getShells();
    if(!active||typeof active!=='object'||Array.isArray(active)||!Array.isArray(inbox)||inbox.length>4096)throw Error('Native completion activity/inbox unknown');
    const quiet=!Object.hasOwn(active,sessionID)&&inbox.length===0&&runningShells(shells,worktree).length===0;
    observation={session,active:Object.hasOwn(active,sessionID),inbox_count:inbox.length,shells,quiet};
    const complete=quiet&&previous===session.time.updated&&session.outcome;
    previous=quiet?session.time.updated:undefined;
    return complete?session:{...session,outcome:undefined};
  }});
}
