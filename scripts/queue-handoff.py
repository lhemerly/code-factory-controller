#!/usr/bin/env python3
"""One-shot cooperative upgrade of an existing queue owner. Never stops its worker."""
import argparse, ctypes, hashlib, importlib.util, json, os, pathlib, select, signal, sys
ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('dispatch',ROOT/'research-dispatch.py');D=importlib.util.module_from_spec(spec);spec.loader.exec_module(D)

def stopped_owner(record,state_path):
 """Freeze only a proven idle coordinator, recheck persisted admission intent, then retire it."""
 if not D.same_process(record):return
 try:fd=os.pidfd_open(record['pid'])
 except ProcessLookupError:return
 try:
  if not D.same_process(record):return
  signal.pidfd_send_signal(fd,signal.SIGSTOP)
  try:
   state=json.loads(pathlib.Path(state_path).read_text())
   if state.get('current') or state.get('status') not in ['paused','complete']:raise RuntimeError('Owner advanced before handoff; preserve it')
   signal.pidfd_send_signal(fd,signal.SIGTERM)
  finally:
   try:signal.pidfd_send_signal(fd,signal.SIGCONT)
   except ProcessLookupError:pass
  select.select([fd],[],[])
 finally:os.close(fd)



def main():
 p=argparse.ArgumentParser();p.add_argument('--manifest',type=pathlib.Path,required=True);p.add_argument('--queue',type=pathlib.Path,required=True);p.add_argument('--runtime-pins',type=pathlib.Path,required=True);a=p.parse_args()
 pins=json.loads(a.runtime_pins.read_text());D.queue_api.pins(pins['files']);target=pathlib.Path(pins['dispatcher']);queue=D.queue_api.Queue(a.queue,D.save);locks=D.Locks();locks.acquire(queue.root/'handoff.lock');owned={};watch=None
 def stopping(*args):raise KeyboardInterrupt('Owner cancelled queued handoff')
 signal.signal(signal.SIGTERM,stopping)
 libc=ctypes.CDLL(None,use_errno=True);libc.inotify_init1.argtypes=[ctypes.c_int];libc.inotify_init1.restype=ctypes.c_int;libc.inotify_add_watch.argtypes=[ctypes.c_int,ctypes.c_char_p,ctypes.c_uint32];libc.inotify_add_watch.restype=ctypes.c_int
 try:
  watch=libc.inotify_init1(os.O_CLOEXEC)
  if watch<0:raise OSError(ctypes.get_errno(),'inotify_init1')
  if libc.inotify_add_watch(watch,str(a.manifest.parent).encode(),0x00000008|0x00000080|0x00000100)<0:raise OSError(ctypes.get_errno(),'inotify_add_watch')
  while True:
   manifest=D.queue_api.read(a.manifest);state=D.queue_api.read(manifest['state']);owner=D.queue_api.read(queue.root/'owner.json');record=owner.get('process')
   for goal in manifest['goals']:
    if state['goals'][goal['id']]['state'] in ['paused','complete']:continue
    path=pathlib.Path(goal['root'])/'PAUSE'
    if not path.exists():
     text='Cooperative queue handoff '+str(os.getpid())+'; finish the owned cycle before maintenance.\n';path.write_text(text);owned[goal['id']]=text
   D.save(queue.root/'handoff.json',{'state':'waiting_for_boundary','process':{k:v for k,v in D.identity(os.getpid()).items() if k!='argv'},'prior_owner':record,'owned_pauses':owned,'runtime_pins':str(a.runtime_pins)})
   if not record or not D.same_process(record):break
   if not state.get('current') and state['status'] in ['paused','complete'] and all(g['state'] in ['paused','complete'] for g in state['goals'].values()):
    try:stopped_owner(record,manifest['state']);break
    except RuntimeError:pass # A new admission raced the idle observation; wait for its boundary.
   os.read(watch,65536) # Kernel event, not timer/model polling.
  held=D.Locks()
  try:
   held.acquire(queue.root/'owner.lock');held.acquire(manifest['pool_lock'])
   manifest=D.queue_api.read(a.manifest);dispatcher=D.Dispatcher(manifest,manifest['state']);dispatcher.recover_and_audit()
   selected=[]
   for goal in manifest['goals']:
    path=pathlib.Path(goal['root'])/'PAUSE'
    if goal['id'] in owned and path.exists() and path.read_text()==owned[goal['id']]:selected.append(goal)
   D.owner_resume({**manifest,'goals':selected,'legacy_idle_owner':None})
   for goal in selected:
    if dispatcher.s['goals'][goal['id']]['state']=='paused':dispatcher.s['goals'][goal['id']]['state']='ready'
   dispatcher.persist();D.save(queue.root/'handoff.json',{'state':'starting_replacement','owned_pauses_released':[g['id'] for g in selected],'dispatcher':str(target)})
  finally:held.close()
 finally:
  if watch is not None and watch>=0:os.close(watch)
  locks.close()
 D.queue_api.pins(pins['files']);os.execv(sys.executable,[sys.executable,str(target),'run','--manifest',str(a.manifest),'--queue',str(a.queue),'--wait-for-owner'])
if __name__=='__main__':main()
