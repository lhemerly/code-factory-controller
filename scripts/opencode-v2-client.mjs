// Operator-selected installed @opencode/client2.0.29; never download or install here.
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const explicit=process.env.FACTORY_OPENCODE_V2_CLIENT;
const location=explicit?pathToFileURL(resolve(explicit)).href:new URL('../../opencode-pstack/node_modules/@opencode/client/dist/promise/index.js',import.meta.url).href;
let client;
try{client=await import(location)}catch(error){throw Error('Install the pinned @opencode/client2.0.29 in an owner-controlled directory and set FACTORY_OPENCODE_V2_CLIENT to its dist/promise/index.js; see docs/research-dispatch.md',{cause:error})}
if(typeof client.OpenCode?.make!=='function')throw Error('Selected OpenCode client has no V2 OpenCode.make API');
export const OpenCode=client.OpenCode;
