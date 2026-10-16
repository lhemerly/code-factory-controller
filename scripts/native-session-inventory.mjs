// OpenCode can return a continuation cursor even on its final nonempty page.
export async function completeSessionInventory(client){
 const data=[],seen=new Set(),cursors=new Set();let cursor;
 for(let page=0;page<32;page++){
  const result=await client.session.list({limit:100,...(cursor?{cursor}:{})});
  if(!Array.isArray(result.data)||result.data.length>100||!result.cursor||typeof result.cursor!=='object')throw Error('Native recovery inventory unknown');
  for(const entry of result.data){if(typeof entry.id!=='string'||seen.has(entry.id))throw Error('Native recovery inventory inconsistent');seen.add(entry.id);data.push(entry)}
  const next=result.cursor.next;if(next==null)return data;
  if(typeof next!=='string'||!next||cursors.has(next))throw Error('Native recovery cursor inconsistent');cursors.add(next);cursor=next;
 }
 throw Error('Native recovery inventory exceeds bound');
}
