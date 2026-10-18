// Completion is an owned native observation, never the session's cached outcome alone.
import {observeResearch,runningShells} from './continuous-ownership.mjs';
import {classify} from './free-model-policy.mjs';
export function classifyNativeFailure(text,phase) {
  if(phase!=='native_sealing')return classify(text,phase);
  const guarded=classify(text,'native');
  if(guarded==='denied'||/permission.?denied|scope violation|ownership.*(?:changed|unknown)/i.test(text))return guarded;
  // Only a qualified local control transport failure authorizes stop recovery.
  // A denial, pricing/accounting uncertainty or schema error never does.
  if(/timeout|timed.out|V2 observation deadline exceeded|ECONN|Connection refused|fetch failed|HTTP 50[234]/i.test(text))return 'repair';
  return 'blocked';
}
export async function awaitNativeCompletion({sessionID,worktree,getSession,getActive,getInbox,getShells,checkpoint,...controls}) {
  let previous,observation,confirmed=false,saved=-Infinity,checked=-Infinity;
  const now=controls.now??Date.now;
  const get=async()=>{
    const session=await getSession();
    if(session.id!==sessionID||session.location?.directory!==worktree||!Number.isFinite(session.time?.updated))throw Error('Native completion ownership/update unknown');
    const active=await getActive(),inbox=await getInbox(),shells=await getShells();
    if(!active||typeof active!=='object'||Array.isArray(active)||!Array.isArray(inbox)||inbox.length>4096)throw Error('Native completion activity/inbox unknown');
    const quiet=!Object.hasOwn(active,sessionID)&&inbox.length===0&&runningShells(shells,worktree).length===0;
    observation={session,active:Object.hasOwn(active,sessionID),inbox_count:inbox.length,shells,quiet};
    confirmed=quiet&&previous===session.time.updated&&Boolean(session.outcome);
    previous=quiet?session.time.updated:undefined;
    // The outcome is cached across automatic continuation. Only actual active
    // execution is nonterminal for failure policy; a quiet first confirmation
    // retains terminal semantics. The checkpoint keeps the original observation.
    return observation.active?{...session,outcome:undefined}:session;
  };
  while(!controls.shouldStop()) {
    const session=await observeResearch({...controls,get,
      checkpoint:async()=>{if(now()-saved>=15000){await checkpoint(observation);saved=now()}},
      resources:async()=>{if(now()-checked>=60000){await controls.resources();checked=now()}}});
    if(confirmed)return session;
    await controls.sleep(2000);
  }
  throw Error('Operator stopping');
}
