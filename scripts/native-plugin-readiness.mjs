// A bounded startup gate, never a model/work scheduler.
export async function waitForFactoryPlugin(client, {timeoutMs=30000,now=Date.now,sleep=ms=>new Promise(ok=>setTimeout(ok,ms)),checkpoint=async()=>{}}={}){
 const deadline=now()+timeoutMs;let observed;
 while(now()<deadline){
  try{
   const commands=(await client.command.list()).data;
   const plugin=(await client.plugin.list()).data.find(p=>p.id==='pstack.opencode');
   observed={plugin:plugin?.state??null,command_ready:commands.some(c=>c.name==='pstack-factory-start')};
   await checkpoint(observed);
   if(plugin?.state && ['error','failed','disabled'].includes(plugin.state.status)){
    const err=new Error('pstack plugin initialization failed: '+JSON.stringify(plugin.state));
    err.code='pstack_plugin_failed';
    throw err;
   }
   if(plugin?.state?.status==='active'&&observed.command_ready)return plugin;
  }catch(err){
   if(isTransientObservation(err)) {
    observed=observed??{plugin:null,command_ready:false};
    await checkpoint(observed);
    if(now()>=deadline) throw err;
    await sleep(100);
    continue;
   }
   throw err;
  }
  await sleep(100);
 }
 throw Error('pstack plugin readiness timed out: '+JSON.stringify(observed));
}

function isTransientObservation(err){
 if(!err)return false;
 if(err.code==='pstack_plugin_failed')return false;
 if(isPermissionDenied(err))return false;
 const status=err.status ?? err.statusCode;
 if(status===503)return true;
 if(err.code==='ECONNRESET')return true;
 const msg=String(err.message||'').toLowerCase();
 if(msg.includes('connection reset')||msg.includes('econnreset'))return true;
 if(msg.includes('service unavailable'))return true;
 return false;
}

function isPermissionDenied(err){
 const status=err.status ?? err.statusCode;
 if(status===401||status===403||status===13)return true;
 const code=String(err.code||'').toUpperCase();
 return code==='EACCES'||code==='EPERM';
}
