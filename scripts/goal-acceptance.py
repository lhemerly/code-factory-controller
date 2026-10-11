"""Host-approved final acceptance in a fresh checkout of the delivered commit."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time
import uuid


def validate_evidence(evidence, check, commit):
    if (evidence.get('passed') is not True or evidence.get('skipped', False)
            or evidence.get('commit') != commit
            or evidence.get('evidence_type') != check['evidence_type']):
        raise ValueError('acceptance must pass without skips for the exact commit and evidence type')
    if check['evidence_type'] == 'vulkan_execution':
        device = evidence.get('device')
        if (type(evidence.get('gpu_dispatches')) is not int or evidence['gpu_dispatches'] <= 0
                or evidence.get('validation_passed') is not True
                or not isinstance(device, dict) or not device.get('name')
                or not all(k in device for k in ('vendor_id', 'device_id'))):
            raise ValueError('Vulkan acceptance requires validated device dispatches and device identity')
    return evidence


def run_acceptance(manifest, state_path, commit):
    checks = manifest.get('completion_checks', [])
    required = set(manifest.get('required_evidence', []))
    if not checks or not required or not required.issubset({c['evidence_type'] for c in checks}):
        return {'passed': False, 'status': 'awaiting_acceptance', 'reason': 'mandatory completion checks/evidence are missing'}
    repository = Path(manifest['repository'])
    if not repository.is_absolute() or not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', commit):
        raise ValueError('absolute repository and exact commit required')
    ids = [c['id'] for c in checks]
    if len(set(ids)) != len(ids):
        raise ValueError('completion check IDs must be unique')
    allowed = {'PATH', 'RUSTUP_HOME', 'CARGO_HOME', 'RUSTUP_TOOLCHAIN', 'VK_ICD_FILENAMES', 'VK_DRIVER_FILES', 'XDG_RUNTIME_DIR'}
    extra = manifest.get('verification_environment', {})
    if not set(extra).issubset(allowed):
        raise ValueError('unsupported verification environment key')
    env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'CI': 'true',
           'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null', **extra,
           'FACTORY_VERIFICATION_COMMIT': commit}
    output = Path(str(state_path) + '.acceptance') / uuid.uuid4().hex
    output.mkdir(parents=True)
    checkout = output / 'checkout'
    subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', 'clone', '--quiet', '--no-hardlinks', '--no-checkout', str(repository), str(checkout)], env=env, check=True, timeout=60)
    subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', 'checkout', '--quiet', '--detach', commit], cwd=checkout, env=env, check=True, timeout=60)
    receipts = []
    for index, check in enumerate(checks):
        argv, timeout = check['argv'], check.get('timeout_seconds', 300)
        if not argv or not all(isinstance(a, str) and a for a in argv) or not 1 <= timeout <= 3600:
            raise ValueError('invalid completion command/timeout')
        stdout, stderr = output / f'{index}.stdout', output / f'{index}.stderr'
        with stdout.open('wb') as out, stderr.open('wb') as err:
            process = subprocess.Popen(argv, cwd=checkout, env=env, stdout=out, stderr=err, start_new_session=True)
            deadline = time.monotonic() + timeout
            failure = None
            while process.poll() is None:
                if time.monotonic() > deadline or stdout.stat().st_size + stderr.stat().st_size > 1024*1024:
                    failure = 'timeout or log limit'
                    os.killpg(process.pid, signal.SIGKILL)
                    break
                time.sleep(.05)
            exit_status = process.wait()
        receipt = {'check_id': check['id'], 'commit': commit, 'exit_status': exit_status,
                   'stdout': str(stdout), 'stderr': str(stderr),
                   'stdout_sha256': hashlib.sha256(stdout.read_bytes()).hexdigest()}
        receipts.append(receipt)
        try:
            if exit_status or failure:
                raise ValueError(failure or 'command failed')
            receipt['evidence'] = validate_evidence(json.loads(stdout.read_text()), check, commit)
        except (ValueError, KeyError) as error:
            return {'passed': False, 'status': 'acceptance_failed', 'reason': str(error), 'commit': commit, 'receipts': receipts}
    return {'passed': True, 'status': 'passed', 'commit': commit, 'receipts': receipts}
