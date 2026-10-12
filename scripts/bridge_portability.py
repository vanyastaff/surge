#!/usr/bin/env python3
"""Run native ACP tests and retain revision-bound evidence using only stdlib."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time
import xml.etree.ElementTree as ET

REQUIRED_TESTS = [
    'crash_after_n_tool_calls_surfaces_within_2s',
    'close_against_stuck_mock_times_out',
    'five_concurrent_sessions_complete_independently',
    'shutdown_emits_session_ended_for_each_open_session',
    'cancelled_open_settles_worker_and_child',
    'bare_drop_during_opening_is_bounded_and_reaps',
    'mock_executable_discovery_uses_native_paths',
]


def run_command(command, root, output, name, timeout, overrides=None):
    started = time.monotonic()
    receipt = {'command': command, 'timed_out': False, 'environment_overrides': overrides or {}}
    environment = dict(os.environ)
    environment.update(overrides or {})
    try:
        process = subprocess.run(command, cwd=root, capture_output=True, text=True,
                                 encoding="utf-8", errors="replace", env=environment, timeout=timeout, check=False)
        code, stdout, stderr = process.returncode, process.stdout, process.stderr
    except subprocess.TimeoutExpired as error:
        receipt['timed_out'] = True
        code, stdout, stderr = 124, error.stdout or '', error.stderr or ''
    except OSError as error:
        code, stdout, stderr = 127, '', str(error)
    for stream, value in [('stdout', stdout), ('stderr', stderr)]:
        if isinstance(value, bytes):
            value = value.decode('utf-8', errors='replace')
        path = output / (name + '.' + stream + '.log')
        path.write_text(value, encoding='utf-8')
        receipt[stream] = path.name
    receipt.update(exit_code=code, elapsed_seconds=round(time.monotonic() - started, 3))
    return receipt


def read_inventory(document, required):
    cases = []
    for suite_name, suite in document['rust-suites'].items():
        for name, test in suite['testcases'].items():
            cases.append((suite_name, name, test['ignored']))
    return {
        'total': len(cases),
        'ignored': [suite + '::' + name for suite, name, ignored in cases if ignored],
        'required_unavailable': [name for name in required
                                 if not any(case == name and not ignored
                                            for _, case, ignored in cases)],
    }


def read_junit(path, required):
    cases = list(ET.parse(path).getroot().iter('testcase'))
    failed = [case for case in cases
              if case.find('failure') is not None or case.find('error') is not None]
    skipped = [case for case in cases if case.find('skipped') is not None]
    passed = [case for case in cases if case not in failed and case not in skipped]
    return {'total': len(cases), 'passed': len(passed), 'failed': len(failed),
            'skipped': len(skipped),
            'required_not_passed': [name for name in required
                                    if not any(case.get('name') == name for case in passed)]}


def revision_errors(before, after, dirty_before, dirty_after):
    errors = []
    if not before or before != after:
        errors.append('HEAD missing or changed during run')
    if dirty_before.strip() or dirty_after.strip():
        errors.append('working tree must be clean before and after run')
    return errors


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--timeout', type=int, default=1200,
                        help='maximum seconds for each build/list/test command')
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    # Reusing a receipt directory could mix revisions or stale successful XML.
    output.mkdir(parents=True, exist_ok=False)
    manifest = {'schema': 1, 'status': 'incomplete', 'errors': [], 'commands': [],
                'started_utc': datetime.now(timezone.utc).isoformat(),
                'host': {'system': platform.system(), 'release': platform.release(),
                         'machine': platform.machine(), 'python': platform.python_version()},
                'required_tests': REQUIRED_TESTS,
                'job_url': None}
    if os.environ.get('GITHUB_RUN_ID'):
        manifest['job_url'] = '{}/{}/actions/runs/{}'.format(
            os.environ.get('GITHUB_SERVER_URL', 'https://github.com'),
            os.environ.get('GITHUB_REPOSITORY', ''), os.environ['GITHUB_RUN_ID'])
    def command(name, words, timeout=60, overrides=None):
        receipt = run_command(words, root, output, name, timeout, overrides)
        manifest['commands'].append(receipt)
        if receipt['exit_code'] != 0:
            manifest['errors'].append(name + ' failed: ' + str(receipt['exit_code']))
        return (output / receipt['stdout']).read_text(encoding='utf-8').strip()
    if os.environ.get('CARGO_BUILD_TARGET'):
        manifest['errors'].append('CARGO_BUILD_TARGET must be unset for native evidence')
    before = command('head-before', ['git', 'rev-parse', 'HEAD'])
    dirty_before = command('status-before', ['git', 'status', '--porcelain', '--untracked-files=all'])
    manifest['revision'] = before
    try:
        manifest['toolchain'] = command('rustc', ['rustc', '-Vv'])
        hosts = [line.removeprefix('host: ').strip()
                 for line in manifest['toolchain'].splitlines() if line.startswith('host: ')]
        if len(hosts) != 1 or not hosts[0]:
            raise ValueError('rustc -Vv must identify exactly one native host')
        host = hosts[0]
        target = Path(os.environ.get('CARGO_TARGET_DIR', root / 'target'))
        if not target.is_absolute():
            target = root / target
        target = target.resolve()
        native_flags = ['--target', host, '--target-dir', str(target)]
        manifest['build_target'] = host
        manifest['target_directory'] = str(target)
        mock = target / host / 'debug' / ('mock_acp_agent.exe' if platform.system() == 'Windows'
                                        else 'mock_acp_agent')
        overrides = {'CARGO_BIN_EXE_mock_acp_agent': str(mock)}
        manifest['cargo'] = command('cargo', ['cargo', '-V'])
        manifest['nextest'] = command('nextest', ['cargo', 'nextest', '--version'])
        if dirty_before:
            manifest['errors'].append('working tree dirty before run')
        if not manifest['errors']:
            command('build', ['cargo', 'build', '--locked', '-p', 'surge-acp',
                              '--all-features', '--bin', 'mock_acp_agent'] + native_flags, args.timeout)
        if not manifest['errors']:
            raw = command('inventory', ['cargo', 'nextest', 'list', '--locked', '-p',
                          'surge-acp', '--all-features', '--message-format', 'json'] + native_flags,
                          args.timeout, overrides)
            manifest['inventory'] = read_inventory(json.loads(raw), REQUIRED_TESTS)
            if manifest['inventory']['required_unavailable']:
                manifest['errors'].append('required tests absent or ignored')
        if not manifest['errors']:
            junit = target / 'nextest' / 'bridge-portability' / 'junit.xml'
            if junit.exists():
                junit.unlink()
            command('tests', ['cargo', 'nextest', 'run', '--locked', '-p', 'surge-acp',
                             '--all-features', '--profile', 'bridge-portability',
                             '--no-fail-fast'] + native_flags, args.timeout, overrides)
            shutil.copyfile(junit, output / 'junit.xml')
            manifest['results'] = read_junit(output / 'junit.xml', REQUIRED_TESTS)
            if manifest['results']['failed'] or manifest['results']['required_not_passed']:
                manifest['errors'].append('required coverage did not pass')
            inventory = manifest['inventory']
            if manifest['results']['passed'] != inventory['total'] - len(inventory['ignored']):
                manifest['errors'].append('executed pass count does not match unignored inventory')
    except (OSError, ValueError, KeyError, TypeError, AttributeError, ET.ParseError) as error:
        manifest['errors'].append(type(error).__name__ + ': ' + str(error))
    finally:
        after = command('head-after', ['git', 'rev-parse', 'HEAD'])
        dirty_after = command('status-after', ['git', 'status', '--porcelain', '--untracked-files=all'])
        manifest['revision_after'] = after
        manifest['errors'].extend(revision_errors(before, after, dirty_before, dirty_after))
        manifest['status'] = 'complete' if not manifest['errors'] else 'incomplete'
        manifest['artifacts'] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                                 for path in output.iterdir() if path.is_file()}
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'status': manifest['status'], 'manifest': str(output / 'manifest.json'),
                      'errors': manifest['errors']}))
    return 0 if manifest['status'] == 'complete' else 1


if __name__ == '__main__':
    raise SystemExit(main())
