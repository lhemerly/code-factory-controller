import {route as freeRoute} from './free-model-policy.mjs';
// Policy selection only, never a permission override or provider identity substitution.
export function chooseProvider({executionAuthorized,quota,nativeVersion,qualifiedVersions=['1.3.6'],models,circuits={},now=Date.now()}) {
 if(!executionAuthorized)return{status:'blocked',reason:'execution authorization unresolved'};
 if(!quota||quota.ai_credits_enabled!==false)return{status:'blocked',reason:'AI Credits OFF must be freshly observed'};
 const age=now-Date.parse(quota.observed_utc);
 if(!Number.isFinite(age)||age<0||age>300000)return{status:'blocked',reason:'included quota observation is stale or invalid'};
 const group=quota.gemini_flash_pro_group;
 const exhausted=group?.weekly_remaining_percent===0||group?.five_hour_remaining_percent===0;
 if(!exhausted){
  if(!Number.isFinite(group?.weekly_remaining_percent)||!Number.isFinite(group?.five_hour_remaining_percent))return{status:'blocked',reason:'included Gemini3.8 quota is unknown'};
  if(!qualifiedVersions.includes(nativeVersion))return{status:'blocked',reason:'agy native version not qualified: '+nativeVersion};
  if(!quota.model_ids?.includes('gemini-3.8-flash-high'))return{status:'blocked',reason:'exact preferred model absent from official catalog'};
  return{status:'selected',provider:'agy',model:'agy/gemini-3.8-flash-high',nativeModel:'gemini-3.8-flash-high',quotaGroup:'Gemini Flash/Pro',timeout_seconds:0};
 }
 const model=freeRoute(models,circuits,now);
 return model?{status:'selected',provider:'opencode_v2',model,reason:'included Gemini3.8 quota exhausted'}:{status:'paused',reason:'all approved free OpenCode models unavailable'};
}
export function controllerProviderConfig(base,selection,agyConfig) {
 if(selection.status!=='selected')throw Error('No authorized compatible provider selected');
 if(selection.provider==='agy')return{...base,policy:{...base.policy,models:[selection.model]},opencode_url:null,agy:{...agyConfig,model:selection.nativeModel,timeout_seconds:0,included_quota_approved:true}};
 const {agy,...rest}=base;return{...rest,policy:{...base.policy,models:[selection.model]}};
}
