// Frozen JS context assertions with the one pre-admission fixture supplied its
// real native auditor. Production callers already supply the pinned controller.
import {readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
const binary=process.env.FACTORY_FIXTURE_CONTROLLER??fileURLToPath(new URL('../target/debug/code-factory-controller',import.meta.url));
let source=await readFile(new URL('./test-research-context.mjs',import.meta.url),'utf8');
source=source.replace("from './research-context.mjs'",'from '+JSON.stringify(new URL('./research-context.mjs',import.meta.url).href));
const fixture="await writeFile(join(f.root,'goal.json'),JSON.stringify(f.input.goal));const next=";
if(!source.includes(fixture))throw Error('Frozen context fixture shape changed');
source=source.replace(fixture,'f.input.cfg.controller='+JSON.stringify(binary)+';'+fixture);
await import('data:text/javascript;base64,'+Buffer.from(source).toString('base64'));
