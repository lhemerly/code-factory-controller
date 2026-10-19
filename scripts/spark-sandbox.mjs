import {resolve,dirname,isAbsolute} from 'node:path';
export function workerPermissions(cfg,work){
 if(!isAbsolute(work)||cfg.data!=null&&(typeof cfg.data!=='string'||!isAbsolute(cfg.data)))throw Error('Worker and optional data paths must be absolute');
 return [{action:'*',resource:'*',effect:'deny'},{action:'pstack.workflow',resource:'feature',effect:'allow'},{action:'pstack.cancel',resource:'*',effect:'allow'},{action:'pstack.factory',resource:'*',effect:'allow'},{action:'read',resource:work+'/**',effect:'allow'},{action:'read',resource:'*',effect:'allow'},{action:'edit',resource:work+'/**',effect:'allow'},{action:'edit',resource:'*',effect:'allow'},{action:'shell',resource:'*',effect:'allow'},
 // This is the private tmpfs created by sandbox(), not the host's /tmp.
 {action:'external_directory',resource:'/tmp',effect:'allow'},{action:'external_directory',resource:'/tmp/**',effect:'allow'},
 // Native read canonicalizes data symlinks and checks this separate category.
 // Grant only the configured mount; sandbox() keeps it read-only at the kernel.
 ...(cfg.data==null?[]:[{action:'external_directory',resource:cfg.data,effect:'allow'},{action:'external_directory',resource:cfg.data+'/**',effect:'allow'}])];
}
export function sandbox(cfg,repo,work,profile,protectedPaths){
 // /etc/resolv.conf may be a regular file on non-systemd hosts.
 // Mount systemd-resolved's backing files read-only when they exist.
 const args=['--unshare-user','--unshare-pid','--unshare-ipc','--unshare-uts','--new-session','--die-with-parent','--cap-drop','ALL','--ro-bind','/usr','/usr','--symlink','usr/bin','/bin','--ro-bind','/lib','/lib','--ro-bind','/lib64','/lib64','--ro-bind','/etc','/etc','--ro-bind-try','/run/systemd/resolve','/run/systemd/resolve','--proc','/proc','--dev','/dev','--tmpfs','/tmp'];
 const resources=[cfg.venv,cfg.data].filter(p=>p!==undefined&&p!==null);
 if(resources.some(p=>typeof p!=='string'||!isAbsolute(p)))throw Error('Configured optional resources must be absolute paths');
 // A linked worker needs Git objects/metadata, not the owner's checkout.
 // Native shell path classification is not a filesystem boundary.
 const repositoryMount=work===repo?repo:resolve(repo,'.git');
 for(const p of [repositoryMount,cfg.plugin,dirname(cfg.native),...resources])args.push('--ro-bind',p,p);
 args.push('--bind',profile,profile);if(work!==repo){args.push('--bind',work,work);for(const p of ['.git','opencode.json','.opencode',...protectedPaths])args.push('--ro-bind',resolve(work,p),resolve(work,p));args.push('--bind',resolve(work,'.opencode/pstack-admission'),resolve(work,'.opencode/pstack-admission'))}
 args.push('--chdir',work,cfg.native);return args;
}
