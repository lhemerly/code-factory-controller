import {resolve,dirname} from 'node:path';
export function sandbox(cfg,repo,work,profile,protectedPaths){
 const args=['--unshare-user','--unshare-pid','--unshare-ipc','--unshare-uts','--new-session','--die-with-parent','--cap-drop','ALL','--ro-bind','/usr','/usr','--ro-bind','/lib','/lib','--ro-bind','/lib64','/lib64','--ro-bind','/etc','/etc','--ro-bind','/run/systemd/resolve','/run/systemd/resolve','--proc','/proc','--dev','/dev','--tmpfs','/tmp'];
 for(const p of [repo,cfg.plugin,dirname(cfg.native),cfg.venv,cfg.data])args.push('--ro-bind',p,p);
 args.push('--bind',profile,profile);if(work!==repo){args.push('--bind',work,work);for(const p of ['.git','opencode.json','.opencode',...protectedPaths])args.push('--ro-bind',resolve(work,p),resolve(work,p));args.push('--bind',resolve(work,'.opencode/pstack-admission'),resolve(work,'.opencode/pstack-admission'))}
 args.push('--chdir',work,cfg.native);return args;
}
