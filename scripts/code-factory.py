#!/usr/bin/env python3
"""Install, update and run the code-factory CLI from published GitHub releases."""
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request

REPO = 'lhemerly/code-factory-controller'
ROOT = Path.home() / '.local/share/code-factory'
BIN = Path.home() / '.local/bin'


def release_tag(value):
    if not re.fullmatch(r'v\d+\.\d+\.\d+', value):
        raise ValueError('only stable semantic-version release tags are supported')
    return value


def atomic_link(target, link):
    temporary = link.with_name(link.name + '.new')
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(target)
    os.replace(temporary, link)


def update(tag=None):
    ROOT.mkdir(parents=True, exist_ok=True)
    with (ROOT / 'update.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if tag is None:
            request = urllib.request.Request(f'https://api.github.com/repos/{REPO}/releases/latest', headers={'User-Agent':'code-factory-updater','Accept':'application/vnd.github+json'})
            with urllib.request.urlopen(request, timeout=30) as response:
                release = json.load(response)
            if release.get('draft') or release.get('prerelease'):
                raise ValueError('latest release must be public and stable')
            tag = release['tag_name']
        tag = release_tag(tag)
        current = ROOT / 'current'
        if current.exists():
            installed = current.resolve().name
            if tuple(map(int, tag[1:].split('.'))) <= tuple(map(int, release_tag(installed)[1:].split('.'))):
                print(f'Already current: {installed}')
                return installed
        versions = ROOT / 'versions'
        versions.mkdir(exist_ok=True)
        destination = versions / tag
        env = {k:v for k,v in os.environ.items() if k in {'PATH','HOME','RUSTUP_HOME','CARGO_HOME','RUSTUP_TOOLCHAIN','TMPDIR'}}
        if not destination.exists():
            with tempfile.TemporaryDirectory(prefix='.install-',dir=versions) as temp:
                source = Path(temp) / 'source'
                subprocess.run(['git','clone','--quiet','--depth','1','--branch',tag,f'https://github.com/{REPO}.git',str(source)],env=env,check=True,timeout=120)
                subprocess.run(['cargo','build','--locked','--release'],cwd=source,env=env,check=True,timeout=1800)
                for test in ('test-goal-runner.py','test-goal-acceptance.py','test-cli-updater.py'):
                    subprocess.run([sys.executable,str(source/'scripts'/test)],env=env,check=True,timeout=60)
                binary = source/'target/release/code-factory-controller'
                subprocess.run([str(binary),'--version'],env=env,check=True,timeout=10)
                staged = Path(temp) / tag
                (staged/'bin').mkdir(parents=True)
                shutil.copy2(binary,staged/'bin/code-factory-controller')
                for name in ('goal-runner.py','goal-acceptance.py','code-factory.py'):
                    shutil.copy2(source/'scripts'/name,staged/'bin'/name)
                (staged/'release.json').write_text(json.dumps({'tag':tag,'repository':REPO,'commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,env=env,text=True).strip()})+'\n')
                os.replace(staged,destination)
        # Existing running services keep their current executable. Only new CLI
        # invocations select the verified replacement; never restart active jobs.
        atomic_link(destination,current)
        BIN.mkdir(parents=True,exist_ok=True)
        wrapper = BIN/'code-factory'
        temporary = wrapper.with_name('.code-factory.new')
        temporary.write_text('#!/bin/sh\nexec python3 "$HOME/.local/share/code-factory/current/bin/code-factory.py" "$@"\n')
        temporary.chmod(0o755)
        os.replace(temporary,wrapper)
        atomic_link(ROOT/'current/bin/code-factory-controller',BIN/'code-factory-controller')
        print(f'Installed {tag}: {wrapper}')
        return tag


def enable_updates():
    units = Path.home()/'.config/systemd/user'
    units.mkdir(parents=True,exist_ok=True)
    (units/'code-factory-update.service').write_text('[Unit]\nDescription=Update code-factory from stable GitHub releases\n\n[Service]\nType=oneshot\nEnvironment=PATH=%h/.cargo/bin:%h/.local/bin:/usr/local/bin:/usr/bin:/bin\nExecStart=%h/.local/bin/code-factory update\n')
    (units/'code-factory-update.timer').write_text('[Unit]\nDescription=Daily code-factory release check\n\n[Timer]\nOnBootSec=10min\nOnCalendar=daily\nRandomizedDelaySec=1h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n')
    subprocess.run(['systemctl','--user','daemon-reload'],check=True)
    subprocess.run(['systemctl','--user','enable','--now','code-factory-update.timer'],check=True)
    print('Automatic stable-release updates enabled (daily).')


def main():
    args = sys.argv[1:]
    if not args or args[0] in ('-h','--help'):
        print('code-factory install [--tag vX.Y.Z] [--auto-update]\ncode-factory update\ncode-factory run --manifest GOAL --state STATE [runner options]\ncode-factory serve/operator/schemas [controller options]\ncode-factory --version')
        return 0
    if args[0] in ('install','update'):
        tag = args[args.index('--tag')+1] if '--tag' in args else None
        update(tag)
        if '--auto-update' in args:
            enable_updates()
        return 0
    binary = ROOT/'current/bin'/('goal-runner.py' if args[0]=='run' else 'code-factory-controller')
    command = [sys.executable,str(binary),*args[1:]] if args[0]=='run' else [str(binary),*args]
    os.execv(command[0],command)

if __name__ == '__main__':
    raise SystemExit(main())
