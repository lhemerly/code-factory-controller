// Read-only pstack transport. Factory adds identity/version fields absent from watcher summaries.
import {spawnSync} from 'node:child_process';
import {closeSync,constants,fsyncSync,openSync,writeFileSync} from 'node:fs';
import {GhGitHubReader,resolveReviewThreads} from '../vendor/pstack-watch-pr/github.ts';
const [repository,number,outputPath]=process.argv.slice(2);
if(!/^[\w.-]+\/[\w.-]+$/.test(repository)||!/^\d+$/.test(number))throw Error('Invalid bound repository/PR');
const [owner,repo]=repository.split('/'),context={owner,repo,number:Number(number)};
function api(path){
 const out=spawnSync('gh',['api','--hostname','github.com',path],{encoding:'utf8',timeout:30000,maxBuffer:4000000});
 if(out.status!==0)throw Error('GitHub read failed: '+(out.stderr??'').slice(0,240));
 return JSON.parse(out.stdout);
}
function pages(path){const result=[];for(let page=1;page<=20;page++){const values=api(path+`?per_page=100&page=${page}`);if(!Array.isArray(values))throw Error('Invalid GitHub page');result.push(...values);if(values.length<100)return result;}throw Error('Review intake exceeds 2000 records; no partial intake');}
const reader=new GhGitHubReader();
const facts=await reader.pullRequest(context),threads=await resolveReviewThreads(reader,context);
const payload=JSON.stringify({facts,threads,pr:api(`repos/${repository}/pulls/${number}`),repository:api(`repos/${repository}`),comments:pages(`repos/${repository}/issues/${number}/comments`),reviews:pages(`repos/${repository}/pulls/${number}/reviews`),inline:pages(`repos/${repository}/pulls/${number}/comments`)});
if(outputPath){
 const bytes=Buffer.from(payload,'utf8');
 if(bytes.byteLength>33554432)throw Error('Snapshot exceeds 32MiB transport limit');
 const fd=openSync(outputPath,constants.O_WRONLY|constants.O_CREAT|constants.O_EXCL|(constants.O_NOFOLLOW??0),0o600);
 try{writeFileSync(fd,bytes);fsyncSync(fd);}finally{closeSync(fd);}
}else{
 console.log(payload);
}

