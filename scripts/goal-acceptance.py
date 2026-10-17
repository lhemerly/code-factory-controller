"""Host-approved final acceptance in a fresh checkout of the delivered commit."""
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import time
import uuid


def validate_evidence(evidence, check, commit):
    if not isinstance(evidence, dict):
        raise ValueError('acceptance evidence must be a JSON object')
    if (evidence.get('passed') is not True
            or evidence.get('commit') != commit
            or evidence.get('evidence_type') != check['evidence_type']):
        raise ValueError('acceptance must pass without skips for the exact commit and evidence type')
    if (('skipped' not in evidence and 'skipped_count' not in evidence)
            or ('skipped' in evidence and evidence['skipped'] is not False)
            or ('skipped_count' in evidence and
                (type(evidence['skipped_count']) is not int or evidence['skipped_count'] != 0))):
        raise ValueError('acceptance must explicitly report no skipped checks')
    # Device-dispatch evidence has its own measured observation contract below.
    # All other check verdicts must measure tests, not merely assert success.
    if check['evidence_type'] != 'vulkan_execution' and not {'executed', 'failed'}.issubset(evidence):
        raise ValueError('acceptance requires measured executed and failed counts')
    for key, positive in [('executed', True), ('failed', False), ('skipped_count', False)]:
        if key in evidence and (type(evidence[key]) is not int
                or (evidence[key] <= 0 if positive else evidence[key] != 0)):
            raise ValueError('acceptance observations must execute tests without failures or skips')
    if check['evidence_type'] == 'vulkan_execution':
        device = evidence.get('device')
        if (type(evidence.get('gpu_dispatches')) is not int or evidence['gpu_dispatches'] <= 0
                or evidence.get('validation_passed') is not True
                or not isinstance(device, dict) or not device.get('name')
                or not all(k in device for k in ('vendor_id', 'device_id'))):
            raise ValueError('Vulkan acceptance requires validated device dispatches and device identity')
    return evidence


def validate_policy(manifest):
    """Check approved prerequisites without launching a verifier or coding work."""
    checks = manifest.get('completion_checks', [])
    required = manifest.get('required_evidence', [])
    if (not isinstance(checks, list) or not checks
            or not isinstance(required, list) or not required
            or not all(isinstance(v, str) and v for v in required)
            or not all(isinstance(c, dict) and isinstance(c.get('id'), str) and c['id']
                       and isinstance(c.get('evidence_type'), str) and c['evidence_type'] for c in checks)
            or not set(required).issubset({c['evidence_type'] for c in checks})):
        raise ValueError('mandatory completion checks/evidence are missing or invalid')
    if len({c['id'] for c in checks}) != len(checks):
        raise ValueError('completion check IDs must be unique')
    for check in checks:
        argv, timeout = check.get('argv'), check.get('timeout_seconds', 300)
        if (not isinstance(argv, list) or not argv
                or not all(isinstance(a, str) and a and '\x00' not in a for a in argv)
                or type(timeout) not in (int, float) or not math.isfinite(timeout) or not 1 <= timeout <= 3600):
            raise ValueError('invalid completion command/timeout')
    repository = manifest.get('repository')
    if not isinstance(repository, str) or not Path(repository).is_absolute():
        raise ValueError('absolute repository required')
    allowed = {'PATH', 'RUSTUP_HOME', 'CARGO_HOME', 'RUSTUP_TOOLCHAIN', 'VK_ICD_FILENAMES', 'VK_DRIVER_FILES', 'XDG_RUNTIME_DIR'}
    extra = manifest.get('verification_environment', {})
    if not isinstance(extra, dict) or not set(extra).issubset(allowed):
        raise ValueError('unsupported verification environment key')
    if any(not isinstance(value, str) or '\x00' in value for value in extra.values()):
        raise ValueError('verification environment values must be strings without null bytes')


def run_acceptance(manifest, state_path, commit):
    receipts = []
    setup = []
    try:
        if manifest.get('verifier_executable') is not None:
            audit_processes(state_path)
        return _run_acceptance(manifest, state_path, commit, receipts, setup)
    except Exception as error:
        # Setup and process-launch failures are acceptance failures too. The
        # runner persists this result instead of leaving acceptance_running.
        phase = getattr(error, 'acceptance_phase', 'policy')
        reason = ''.join(c if c.isprintable() else ' ' for c in str(error))[:1000]
        return {'passed': False, 'status': 'acceptance_failed', 'commit': commit,
                'phase': phase, 'reason': reason, 'error_type': type(error).__name__,
                'execution_unresolved': getattr(error, 'execution_unresolved', False),
                'receipts': receipts, 'setup': setup,
                'retry_guidance': 'Restore the approved repository and verifier prerequisites, inspect retained logs, and resume the same goal state.'}


def audit_processes(state_path):
    """Never replace a prior verifier until its helper has confirmed quiescence."""
    count = 0
    for path in Path(str(state_path)+'.acceptance').glob('*/*.plan.json'):
        try:
            plan = json.loads(path.read_text())
            observed = json.loads(path.with_name(path.name.replace('.plan.json','.process.json')).read_text())
            if (observed.get('version') != 1 or observed.get('execution_id') != plan['execution_id']
                    or observed.get('observer') != 'linux_subreaper_waitpid_pidfd'
                    or observed.get('quiescent') is not True):
                raise ValueError('Prior goal verifier ownership/quiescence unconfirmed')
            count += 1
        except Exception as error:
            error.execution_unresolved = True
            raise
    return count


def audit_logs(receipt, require_stderr=False):
    for name in ('stdout', 'stderr'):
        expected = receipt.get(name+'_sha256')
        # Legacy standalone receipts may lack a stderr digest. They never
        # establish supervised ownership; current supervised receipts need both.
        if name == 'stderr' and expected is None and not require_stderr:
            continue
        if not expected or hashlib.sha256(Path(receipt[name]).read_bytes()).hexdigest() != expected:
            raise ValueError('Goal acceptance evidence changed: '+name)


def audit_result(manifest, result, commit):
    """Revalidate saved completion without executing commands or sending signals.

    Standalone unsupervised receipts remain supported only when the caller has
    not required a verifier executable. Presence of either lifecycle field also
    requires the complete supervised format, so partial records cannot downgrade.
    """
    validate_policy(manifest)
    if (result.get('passed') is not True or result.get('status') != 'passed'
            or result.get('commit') != commit):
        raise ValueError('Goal completion identity changed')
    checks = {check['id']: check for check in manifest['completion_checks']}
    receipts = result.get('receipts', [])
    if (not isinstance(receipts, list) or len(receipts) != len(checks)
            or {r.get('check_id') for r in receipts} != set(checks)):
        raise ValueError('Goal completion check coverage changed')
    for receipt in receipts:
        check = checks[receipt['check_id']]
        supervised = (manifest.get('verifier_executable') is not None
                      or 'executor' in receipt or 'process_receipt' in receipt)
        if receipt.get('commit') != commit or type(receipt.get('exit_status')) is not int or receipt['exit_status'] != 0:
            raise ValueError('Goal completion receipt changed')
        audit_logs(receipt, require_stderr=supervised)
        evidence = validate_evidence(json.loads(Path(receipt['stdout']).read_text()), check, commit)
        if evidence != receipt.get('evidence'):
            raise ValueError('Goal structured evidence changed')
        if not supervised:
            continue
        stdout, stderr = Path(receipt['stdout']).resolve(), Path(receipt['stderr']).resolve()
        process_path = stdout.with_suffix('.process.json')
        if Path(receipt['process_receipt']).resolve() != process_path:
            raise ValueError('Goal process receipt path changed')
        observed = json.loads(process_path.read_text())
        plan = json.loads(stdout.with_suffix('.plan.json').read_text())
        if (observed != receipt.get('executor') or type(observed.get('version')) is not int
                or observed['version'] != 1 or observed.get('observer') != 'linux_subreaper_waitpid_pidfd'
                or type(observed.get('root_exit_status')) is not int or observed['root_exit_status'] != 0
                or observed.get('quiescent') is not True or observed.get('leaked_descendants') is not False
                or observed.get('termination_reason') != 'root_exit' or observed.get('cleanup_error')
                or observed.get('output_truncated') is not False):
            raise ValueError('Goal execution ownership unconfirmed or evidence changed')
        env = {'CI': 'true', 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
               **manifest.get('verification_environment', {})}
        env.pop('PATH', None)
        argv = plan.get('argv', [])
        # JSON object order is not part of the manifest fingerprint. Assignment
        # order is also immaterial to env, but duplicates/extra assignments and
        # changes to the actual command must still fail the binding check.
        assignments = argv[1:1+len(env)] if isinstance(argv, list) else []
        actual_env = dict(value.split('=', 1) for value in assignments
                          if isinstance(value, str) and '=' in value)
        command_matches = (isinstance(argv, list) and argv[:1] == ['/usr/bin/env']
                           and len(assignments) == len(actual_env) == len(env) and actual_env == env
                           and argv[1+len(env):] == check['argv'])
        if (not observed.get('execution_id') or plan.get('execution_id') != observed['execution_id']
                or plan.get('commit') != commit or not command_matches
                or plan.get('seconds') != math.ceil(check.get('timeout_seconds', 300))
                or plan.get('stdout') != str(stdout) or plan.get('log') != str(stderr)
                or plan.get('receipt') != str(process_path) or plan.get('cwd') != str(stdout.parent/'checkout')):
            raise ValueError('Goal process plan binding changed')
    return result


def supervised_check(executable, argv, checkout, env, timeout, stdout, stderr, receipt):
    """Reuse Factory's Linux subreaper executor; keep its control pipe alive.

    Parent loss closes the pipe and asks the existing helper to clean only its
    owned descendants. No process scan, PID replay or second executor is added.
    """
    if not isinstance(executable, str) or not Path(executable).is_absolute():
        raise ValueError('absolute trusted verifier executable required')
    execution_id = uuid.uuid4().hex
    plan_path = stdout.with_suffix('.plan.json')
    process_path = stdout.with_suffix('.process.json')
    plan = {'execution_id': execution_id, 'argv': ['/usr/bin/env',
            *[key+'='+value for key, value in env.items()
              if key not in ('PATH', 'FACTORY_VERIFICATION_COMMIT')], *argv],
            'cwd': str(checkout.resolve()), 'log': str(stderr.resolve()),
            'stdout': str(stdout.resolve()), 'commit': env['FACTORY_VERIFICATION_COMMIT'],
            'seconds': math.ceil(timeout), 'receipt': str(process_path.resolve())}
    plan_path.write_text(json.dumps(plan))
    receipt['process_receipt'] = str(process_path.resolve())
    with stdout.with_suffix('.supervisor.stderr').open('wb') as diagnostic:
        process = subprocess.Popen([executable, 'verifier-supervisor', '--plan', str(plan_path.resolve())],
            env={'PATH': env['PATH']}, stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL, stderr=diagnostic)
        try:
            process.wait(timeout=math.ceil(timeout)+5)
        except subprocess.TimeoutExpired as error:
            error.execution_unresolved = True
            raise
        finally:
            process.stdin.close()
            if process.poll() is None:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired as error:
                    error.execution_unresolved = True
                    raise
    try:
        observed = json.loads(process_path.read_text())
        receipt['executor'] = observed
        if (process.returncode != 0 or observed.get('version') != 1
                or observed.get('execution_id') != execution_id
                or observed.get('observer') != 'linux_subreaper_waitpid_pidfd'
                or observed.get('quiescent') is not True):
            raise ValueError('Goal verifier ownership/quiescence unconfirmed')
    except Exception as error:
        error.execution_unresolved = True
        raise
    receipt['exit_status'] = observed.get('root_exit_status')
    if (observed.get('root_exit_status') != 0 or observed.get('leaked_descendants') is not False
            or observed.get('termination_reason') != 'root_exit' or observed.get('cleanup_error')
            or observed.get('output_truncated') is not False):
        return observed.get('termination_reason', 'supervised execution failed')
    return None


def _run_acceptance(manifest, state_path, commit, receipts, setup):
    checks = manifest.get('completion_checks', [])
    required = manifest.get('required_evidence', [])
    if not checks or not required:
        return {'passed': False, 'status': 'awaiting_acceptance', 'reason': 'mandatory completion checks/evidence are missing'}
    if (isinstance(required, list) and all(isinstance(v, str) for v in required)
            and isinstance(checks, list) and all(isinstance(c, dict) and isinstance(c.get('evidence_type'), str) for c in checks)
            and not set(required).issubset({c['evidence_type'] for c in checks})):
        return {'passed': False, 'status': 'awaiting_acceptance', 'reason': 'mandatory completion checks/evidence are missing'}
    validate_policy(manifest)
    repository = Path(manifest['repository'])
    if not repository.is_absolute() or not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', commit):
        raise ValueError('absolute repository and exact commit required')
    extra = manifest.get('verification_environment', {})
    env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'CI': 'true',
           'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null', **extra,
           'FACTORY_VERIFICATION_COMMIT': commit}
    try:
        output = Path(str(state_path) + '.acceptance') / uuid.uuid4().hex
        output.mkdir(parents=True)
        checkout = output / 'checkout'
        for stage, argv, cwd in [
            ('clone', ['git', '-c', 'core.hooksPath=/dev/null', 'clone', '--quiet', '--no-hardlinks', '--no-checkout', str(repository), str(checkout)], None),
            ('checkout', ['git', '-c', 'core.hooksPath=/dev/null', 'checkout', '--quiet', '--detach', commit], checkout),
        ]:
            stdout, stderr = output / f'{stage}.stdout', output / f'{stage}.stderr'
            receipt = {'stage': stage, 'stdout': str(stdout), 'stderr': str(stderr), 'exit_status': None}
            setup.append(receipt)
            with stdout.open('wb') as out, stderr.open('wb') as err:
                result = subprocess.run(argv, cwd=cwd, env=env, stdout=out, stderr=err, timeout=60)
            receipt['exit_status'] = result.returncode
            if result.returncode:
                raise ValueError(f'acceptance {stage} failed; inspect retained setup logs')
    except Exception as error:
        error.acceptance_phase = 'setup'
        raise
    for index, check in enumerate(checks):
        argv, timeout = check['argv'], check.get('timeout_seconds', 300)
        stdout, stderr = output / f'{index}.stdout', output / f'{index}.stderr'
        receipt = {'check_id': check['id'], 'commit': commit, 'exit_status': None,
                   'stdout': str(stdout), 'stderr': str(stderr)}
        receipts.append(receipt)
        try:
            if manifest.get('verifier_executable') is not None:
                failure = supervised_check(manifest['verifier_executable'], argv, checkout,
                    env, timeout, stdout, stderr, receipt)
            else:
                with stdout.open('wb') as out, stderr.open('wb') as err:
                    process = subprocess.Popen(argv, cwd=checkout, env=env, stdout=out, stderr=err, start_new_session=True)
                    try:
                        deadline = time.monotonic() + timeout
                        failure = None
                        while process.poll() is None:
                            if time.monotonic() > deadline or stdout.stat().st_size + stderr.stat().st_size > 1024*1024:
                                failure = 'timeout or log limit'
                                break
                            time.sleep(.05)
                    finally:
                        if process.poll() is None:
                            try:
                                os.killpg(process.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        receipt['exit_status'] = process.wait()
            receipt['stdout_sha256'] = hashlib.sha256(stdout.read_bytes()).hexdigest()
            receipt['stderr_sha256'] = hashlib.sha256(stderr.read_bytes()).hexdigest()
        except Exception as error:
            error.acceptance_phase = 'check_launch'
            raise
        try:
            exit_status = receipt['exit_status']
            if exit_status or failure:
                raise ValueError(failure or 'command failed')
            receipt['evidence'] = validate_evidence(json.loads(stdout.read_text()), check, commit)
        except Exception as error:
            error.acceptance_phase = 'check_evidence'
            raise
    return {'passed': True, 'status': 'passed', 'commit': commit, 'receipts': receipts, 'setup': setup}
