#!/usr/bin/env python3
"""Local host controls. Does not send model requests or alter Factory receipts."""
import argparse, datetime, fcntl, json, os, pathlib, tempfile
p=argparse.ArgumentParser();p.add_argument('action',choices=['status','pause','resume']);p.add_argument('--root',required=True,type=pathlib.Path);p.add_argument('--reason');a=p.parse_args();r=a.root.resolve()
if a.action=='status':
 print((r/'status.json').read_text());raise SystemExit
if a.action=='pause':
 (r/'PAUSE').write_text((a.reason or 'Operator pause')+'\n');print('Pause requested at next cycle boundary. For immediate owned-worker fencing use systemctl --user stop spark-code-factory.service.');raise SystemExit
if not a.reason:p.error('resume requires an explicit --reason after inspecting recovery evidence')
with (r/'supervisor.lock').open('a') as lock:
 try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 except BlockingIOError:raise SystemExit('Stop the owned service first; resume cannot race a live supervisor')
 s=json.loads((r/'status.json').read_text())
 if s.get('current'):raise SystemExit('Unresolved active cycle; recover ownership/stop before resume. No automatic override.')
 (r/'PAUSE').unlink(missing_ok=True);s['status']='waiting';s['pause_reason']=None;s['operator_resume']={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'reason':a.reason}
 fd,name=tempfile.mkstemp(dir=r,prefix='.resume-')
 with os.fdopen(fd,'w') as f:json.dump(s,f,indent=2);f.write('\n');f.flush();os.fsync(f.fileno())
 os.replace(name,r/'status.json')
 with (r/'events.ndjson').open('a') as f:f.write(json.dumps({'kind':'operator_resume',**s['operator_resume']})+'\n');f.flush();os.fsync(f.fileno())
 print('Resume prepared. Provider-denial circuits and old receipts remain unchanged. Start the user service.')
