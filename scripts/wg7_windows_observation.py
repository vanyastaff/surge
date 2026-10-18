#!/usr/bin/env python3
"""Fixed WG7 Windows observation; the unchanged full suite remains mandatory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import xml.etree.ElementTree as ET

from bridge_portability import read_junit, revision_errors, run_command

CASES = (
    ('surge-daemon::quota_recovery_route_test', 'actual_429_after_declared_source_change_is_audit_only_and_primary_is_attempted_again'),
    ('surge-daemon::owned_flow_mcp_recovery', 'explicit_empty_survives_real_cold_wake_and_changed_global_defaults'),
    ('surge-daemon::owned_flow_mcp_recovery', 'populated_cold_valid_twin_keeps_original_transport_and_writer_history'),
    ('surge-daemon::work_item_route_test', 'cold_daemon_resumes_committed_startup_with_same_run_and_accepted_acp_context'),
    ('surge-daemon::work_item_route_test', 'task_split_recovery::accepted_split_recovers_children_once_with_fresh_retry_budget'),
    ('surge-orchestrator', 'engine::ipc::windows_address_tests::public_resolver_aliases_reach_same_actual_listener_with_ping'),
)
BASE = ['--locked', '--workspace', '--exclude', 'surge-ui']
PROFILE = 'wg7-windows-observation'
FILTER = ' | '.join(f'(binary_id(={binary}) & test(={name}))' for binary, name in CASES)
OUTCOMES = ('checker', 'focused', 'early_upload', 'full', 'identity', 'final_upload')


def inventory_errors(document):
    selected, binaries, errors = [], {}, []
    for suite in document['rust-suites'].values():
        for name, case in suite['testcases'].items():
            if case['filter-match']['status'] != 'matches':
                continue
            key = (suite['binary-id'], name)
            selected.append(key)
            if case['ignored']:
                errors.append('selected test is ignored: ' + repr(key))
            binaries[suite['binary-id']] = suite['binary-path']
    if len(selected) != 6 or set(selected) != set(CASES):
        errors.append('selected binary/test identities must be exactly the fixed six')
    return errors, binaries


def junit_errors(path):
    errors = []
    summary = read_junit(path, [name for _, name in CASES])
    identities = []
    for suite in ET.parse(path).getroot().iter('testsuite'):
        for case in suite.findall('testcase'):
            identities.append((suite.get('name'), case.get('name')))
            if case.get('classname') != suite.get('name'):
                errors.append('JUnit testcase classname differs from its binary suite')
    if len(identities) != 6 or set(identities) != set(CASES):
        errors.append('JUnit executed identities must be exactly the fixed six')
    if summary['passed'] != 6 or summary['failed'] or summary['skipped']:
        errors.append('all six actual JUnit executions must pass without skips')
    if summary['required_not_passed']:
        errors.append('JUnit required passing executions missing')
    return errors, summary


def file_identity(path):
    path = Path(path)
    if path.is_symlink() or not path.is_file():
        raise ValueError('selected binary is not a regular non-symlink file: ' + str(path))
    return {'path': str(path.resolve()), 'bytes': path.stat().st_size,
            'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def identity_errors(before, after):
    errors = revision_errors(before['revision'], after['revision'], before['dirty'], after['dirty'])
    if before['revision'] != before['github_sha'] or after['revision'] != before['github_sha']:
        errors.append('observed HEAD differs from the workflow SHA')
    if before['binaries'] != after['binaries']:
        errors.append('selected test/mock binary identities changed')
    return errors


def outcome_errors(outcomes):
    if set(outcomes) != set(OUTCOMES):
        return ['exact required step outcomes are missing or unexpected']
    return [name + ' actual outcome is ' + str(value)
            for name, value in outcomes.items() if value != 'success']


def command(root, output, receipts, name, words, timeout=60):
    receipt = run_command(words, root, output, name, timeout)
    receipts.append(receipt)
    if receipt['exit_code'] != 0:
        raise ValueError(name + ' failed: ' + str(receipt['exit_code']))
    return (output / receipt['stdout']).read_text(encoding='utf-8').strip()


def snapshot(root, output, receipts, bindings, label):
    return {
        'revision': command(root, output, receipts, label + '-head', ['git', 'rev-parse', 'HEAD']),
        'dirty': command(root, output, receipts, label + '-status', ['git', 'status', '--porcelain', '--untracked-files=all']),
        'github_sha': os.environ.get('GITHUB_SHA', ''),
        'binaries': {name: file_identity(path) for name, path in bindings.items()},
    }


def capture(root, output, manifest):
    if sys.platform != 'win32' or os.environ.get('CARGO_BUILD_TARGET'):
        raise ValueError('fixed observation requires native Windows without CARGO_BUILD_TARGET')
    receipts = manifest['commands']
    raw = command(root, output, receipts, 'inventory',
                  ['cargo', 'nextest', 'list', *BASE, '--message-format', 'json', '-E', FILTER], 1200)
    inventory = json.loads(raw)
    errors, bindings = inventory_errors(inventory)
    manifest['errors'].extend(errors)
    target = Path(inventory['rust-build-meta']['target-directory'])
    bindings['mock_acp_agent'] = str(target / 'debug' / 'mock_acp_agent.exe')
    manifest['bindings'] = bindings
    manifest['before'] = snapshot(root, output, receipts, bindings, 'before')
    manifest['errors'].extend(identity_errors(manifest['before'], manifest['before']))
    if manifest['errors']:
        return
    # A failed command must never inherit a successful XML from an earlier run.
    junit = root / 'target' / 'nextest' / PROFILE / 'wg7-windows-observation.xml'
    junit.unlink(missing_ok=True)
    receipt = run_command(['cargo', 'nextest', 'run', *BASE, '--profile', PROFILE,
                           '--test-threads', '1', '--no-fail-fast', '-E', FILTER],
                          root, output, 'focused', 1200)
    receipts.append(receipt)
    if receipt['exit_code'] != 0:
        manifest['errors'].append('focused nextest failed: ' + str(receipt['exit_code']))
    shutil.copyfile(junit, output / 'focused.junit.xml')
    errors, manifest['junit'] = junit_errors(output / 'focused.junit.xml')
    manifest['errors'].extend(errors)
    manifest['after_focus'] = snapshot(root, output, receipts, bindings, 'after-focus')
    manifest['errors'].extend(identity_errors(manifest['before'], manifest['after_focus']))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=('capture', 'verify', 'enforce'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--outcomes', help='fixed workflow step actual outcomes, as JSON')
    args = parser.parse_args(argv)
    output = args.output.resolve()
    root = Path(__file__).resolve().parent.parent
    if args.phase == 'enforce':
        errors = outcome_errors(json.loads(args.outcomes or '{}'))
        for error in errors:
            print(error)
        return int(bool(errors))
    manifest = {'schema': 1, 'phase': args.phase, 'errors': [], 'commands': [],
                'required_cases': CASES}
    destination = output / (args.phase + '.json')
    if args.phase == 'capture':
        try:
            output.mkdir(parents=True, exist_ok=False)
        except OSError as error:
            print(str(error))
            return 1  # Never overwrite an earlier observation directory.
    try:
        if args.phase == 'capture':
            capture(root, output, manifest)
        else:
            previous = json.loads((output / 'capture.json').read_text(encoding='utf-8'))
            manifest['after_full'] = snapshot(root, output, manifest['commands'], previous['bindings'], 'after-full')
            manifest['errors'].extend(identity_errors(previous['before'], manifest['after_full']))
            manifest['errors'].extend(previous['errors'])
    except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
        manifest['errors'].append(str(error))
    if output.is_dir():
        destination.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(manifest, indent=2))
    return int(bool(manifest['errors']))


if __name__ == '__main__':
    raise SystemExit(main())
