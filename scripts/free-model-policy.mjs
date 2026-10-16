// Host routing policy only. Native OpenCode owns execution; Factory owns acceptance.
export function freeModels(models) {
  return models.filter(m => m.providerID === 'opencode' && m.enabled === true && m.status === 'active'
    && Array.isArray(m.cost) && m.cost.length > 0
    && m.cost.every(t => t.input === 0 && t.output === 0 && t.cache?.read === 0 && t.cache?.write === 0))
    .map(m => ({id:m.providerID+'/'+m.id,cost:m.cost,name:m.name}))
    .sort((a,b) => a.id === 'opencode/big-pickle' ? -1 : b.id === 'opencode/big-pickle' ? 1 : a.id.localeCompare(b.id));
}
export function classify(text, phase='native') {
  // The controller's structured test verdict is not an access denial embedded in a task/log.
  if (phase === 'verification' && /^Error: Independent verification failed: /.test(text)) return 'repair';
  if (/permission.?denied|requires?.?approval|scope violation|ownership.*(?:changed|unknown)|V2 active or unknown shell work|V2 shell.*(?:scope|unknown)/i.test(text)) return 'blocked';
  if (phase === 'verification') return 'repair';
  if (/Factory model has nonzero or unknown pricing/.test(text)) return 'unavailable';
  if (/403|401|free.?tier|authentication|unauthorized|forbidden/i.test(text)) return 'denied';
  if (/quota|429|rate.?limit|capacity|unavailable|overloaded|model.*not.?found|unsupported.*model|does not support.*tool/i.test(text)) return 'unavailable';
  if (/timeout|timed.out|ECONN|fetch failed|502|503|504|reset by peer/i.test(text)) return 'transient';
  return 'blocked';
}
export function route(models, circuits, now) {
  return models.find(m => !circuits[m.id] || circuits[m.id].until <= now)?.id;
}
export function failure(circuits, model, kind, now) {
  const previous=circuits[model]??{};
  const count=previous.kind===kind?(previous.count??0)+1:1;
  // Access denials stay open until explicit operator reset. Do not probe around them.
  const until=kind==='denied'?Number.MAX_SAFE_INTEGER:now+(kind==='unavailable'?21600000:Math.min(3600000,60000*2**Math.min(count-1,6)));
  return {...circuits,[model]:{kind,count,until}};
}
export function recoveryAction(record) {
  // Lost ownership/stop evidence never authorizes a replacement worker.
  if (!record) return 'new';
  if (record.status==='accepted' && record.cleanup_confirmed) return 'continue';
  if (record.cleanup_confirmed && record.execution_stop==='confirmed') return 'continue';
  return 'pause';
}
export function replacementAllowed(record) {
  return record.cleanup_confirmed === true && !record.fence_error
    && record.recovery?.execution_stop?.status === 'confirmed'
    && record.recovery?.accounting?.status !== 'violating';
}
export function approvedScope(paths, template) {
  const under=(p,root)=>p===root||p.startsWith(root+'/');
  return paths.every(p=>template.allowed_paths.some(root=>under(p,root))&&!template.protected_paths.some(root=>under(p,root)));
}

export function researchContinuation(out) {
  const next='Continue useful research toward the standing scientific objective. Run a defensible experiment or negative control, choose another method when the previous result is inconclusive, and retain exact evidence and actionable missing inputs. Do not repeat a cosmetic documentation patch or treat prose/software checks as a clear scientific win. Preserve frozen tests; place new tests under approved tests/ paths.';
  // Compatibility helper only; admission now uses research-context.mjs and independently inspected receipts.
  // Never carry worker prose, serialized errors or Markdown tail slices forward as instructions.
  return next+(out.status==='accepted'?' Prior software milestone requires fresh context before continuing.':' Any retained candidate is unaccepted repair input.');
}
