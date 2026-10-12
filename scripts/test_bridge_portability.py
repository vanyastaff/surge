"""Contract tests for native bridge evidence receipts (no Rust build)."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import bridge_portability as runner


class EvidenceTests(unittest.TestCase):
    def test_junit_counts_actual_cases_and_requires_passing_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            xml = Path(directory) / 'tests.xml'
            xml.write_text('<testsuites><testsuite tests="99"><testcase name="ok"/>'
                           '<testcase name="bad"><failure/></testcase>'
                           '<testcase name="skip"><skipped/></testcase></testsuite></testsuites>')
            result = runner.read_junit(xml, ['ok', 'bad', 'missing'])
            self.assertEqual((result['passed'], result['failed'], result['skipped']), (1, 1, 1))
            self.assertEqual(result['required_not_passed'], ['bad', 'missing'])

    def test_missing_xml_fails(self):
        with self.assertRaises(FileNotFoundError):
            runner.read_junit(Path('/nonexistent/bridge-evidence.xml'), [])

    def test_inventory_retains_ignored_separately(self):
        inventory = {'rust-suites': {'suite': {'testcases': {
            'ok': {'ignored': False}, 'skip': {'ignored': True}}}}}
        result = runner.read_inventory(inventory, ['ok', 'skip', 'missing'])
        self.assertEqual(result['total'], 2)
        self.assertEqual(result['ignored'], ['suite::skip'])
        self.assertEqual(result['required_unavailable'], ['skip', 'missing'])

    def test_command_failure_preserves_receipt_and_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            completed = subprocess.CompletedProcess(['false'], 7, 'out', 'err')
            with patch.object(runner.subprocess, 'run', return_value=completed):
                receipt = runner.run_command(['false'], Path('.'), output, 'failed', 5)
            self.assertEqual(receipt['exit_code'], 7)
            self.assertEqual((output / 'failed.stdout.log').read_text(), 'out')
            self.assertEqual((output / 'failed.stderr.log').read_text(), 'err')

    def test_revision_and_cleanliness_are_both_required(self):
        self.assertEqual(runner.revision_errors('abc', 'abc', '', ''), [])
        self.assertTrue(runner.revision_errors('abc', 'def', '', ''))
        self.assertTrue(runner.revision_errors('abc', 'abc', ' M source', ''))
        self.assertTrue(runner.revision_errors('abc', 'abc', '', '?? source'))

    def test_main_failed_build_still_writes_incomplete_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'receipt'
            def process(command, **kwargs):
                if command[:3] == ['git', 'rev-parse', 'HEAD']:
                    return subprocess.CompletedProcess(command, 0, 'abc\n', '')
                if command[:2] == ['git', 'status']:
                    return subprocess.CompletedProcess(command, 0, '', '')
                if command[:2] == ['cargo', 'build']:
                    return subprocess.CompletedProcess(command, 8, '', 'build failed')
                if command[:2] == ['rustc', '-Vv']:
                    return subprocess.CompletedProcess(command, 0, 'host: native-test-host\n', '')
                return subprocess.CompletedProcess(command, 0, 'version', '')
            with patch.object(runner.subprocess, 'run', side_effect=process):
                with patch.dict(runner.os.environ, {}, clear=True):
                    code = runner.main(['--output', str(output)])
            document = json.loads((output / 'manifest.json').read_text())
            self.assertEqual(code, 1)
            self.assertEqual(document['status'], 'incomplete')
            self.assertTrue(any(command['exit_code'] == 8 for command in document['commands']))
            self.assertEqual(document['revision_after'], 'abc')
            self.assertIn('build.stderr.log', document['artifacts'])

    def test_native_target_is_explicit_despite_cargo_config_and_inventory_is_guarded(self):
        for inventory in ['null', '{"rust-suites": null}']:
            with self.subTest(inventory=inventory), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / 'receipt'
                observed = []
                def process(command, **kwargs):
                    observed.append((command, kwargs))
                    if command[:3] == ['git', 'rev-parse', 'HEAD']:
                        return subprocess.CompletedProcess(command, 0, 'abc', '')
                    if command[:2] == ['git', 'status']:
                        return subprocess.CompletedProcess(command, 0, '', '')
                    if command[:2] == ['rustc', '-Vv']:
                        return subprocess.CompletedProcess(command, 0, 'host: native-host', '')
                    if command[:3] == ['cargo', 'nextest', 'list']:
                        return subprocess.CompletedProcess(command, 0, inventory, '')
                    return subprocess.CompletedProcess(command, 0, 'version', '')
                with patch.object(runner.subprocess, 'run', side_effect=process):
                    with patch.dict(runner.os.environ, {'CARGO_TARGET_DIR': 'target-custom'}, clear=True):
                        self.assertEqual(runner.main(['--output', str(output)]), 1)
                document = json.loads((output / 'manifest.json').read_text())
                self.assertEqual(document['status'], 'incomplete')
                self.assertTrue(any('TypeError' in error or 'AttributeError' in error
                                    for error in document['errors']))
                native = [(cmd, options) for cmd, options in observed
                          if cmd[:2] == ['cargo', 'build'] or cmd[:3] == ['cargo', 'nextest', 'list']]
                self.assertEqual(len(native), 2)
                for cmd, _ in native:
                    self.assertEqual(cmd[cmd.index('--target') + 1], 'native-host')
                    self.assertEqual(cmd[cmd.index('--target-dir') + 1], document['target_directory'])
                self.assertIn('target-custom', document['target_directory'])
                override = native[-1][1]['env']['CARGO_BIN_EXE_mock_acp_agent']
                self.assertIn('native-host', override)
                self.assertIn('mock_acp_agent', override)

    def test_build_list_and_run_share_native_target_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'receipt'
            target = Path(directory) / 'cargo-target'
            observed = []
            def process(command, **kwargs):
                observed.append((command, kwargs))
                stdout = 'version'
                if command[:3] == ['git', 'rev-parse', 'HEAD']:
                    stdout = 'abc'
                elif command[:2] == ['git', 'status']:
                    stdout = ''
                elif command[:2] == ['rustc', '-Vv']:
                    stdout = 'host: native-host'
                elif command[:3] == ['cargo', 'nextest', 'list']:
                    stdout = json.dumps({'rust-suites': {'suite': {'testcases': {
                        name: {'ignored': False} for name in runner.REQUIRED_TESTS}}}})
                elif command[:3] == ['cargo', 'nextest', 'run']:
                    junit = target / 'nextest' / 'bridge-portability' / 'junit.xml'
                    junit.parent.mkdir(parents=True)
                    junit.write_text('<testsuites><testsuite>' + ''.join(
                        '<testcase name="' + name + '"/>' for name in runner.REQUIRED_TESTS
                    ) + '</testsuite></testsuites>')
                return subprocess.CompletedProcess(command, 0, stdout, '')
            with patch.object(runner.subprocess, 'run', side_effect=process):
                with patch.dict(runner.os.environ, {'CARGO_TARGET_DIR': str(target)}, clear=True):
                    self.assertEqual(runner.main(['--output', str(output)]), 0)
            native = [(cmd, options) for cmd, options in observed
                      if cmd[:2] == ['cargo', 'build'] or
                      cmd[:3] in [['cargo', 'nextest', 'list'], ['cargo', 'nextest', 'run']]]
            self.assertEqual(len(native), 3)
            for cmd, _ in native:
                self.assertEqual(cmd[cmd.index('--target') + 1], 'native-host')
                self.assertEqual(cmd[cmd.index('--target-dir') + 1], str(target.resolve()))
            self.assertEqual(native[1][1]['env']['CARGO_BIN_EXE_mock_acp_agent'],
                             native[2][1]['env']['CARGO_BIN_EXE_mock_acp_agent'])

    def test_timeout_preserves_partial_output(self):
        with tempfile.TemporaryDirectory() as directory:
            error = subprocess.TimeoutExpired(['slow'], 1, output=b'partial', stderr=b'waiting')
            with patch.object(runner.subprocess, 'run', side_effect=error):
                result = runner.run_command(['slow'], Path('.'), Path(directory), 'slow', 1)
            self.assertTrue(result['timed_out'])
            self.assertNotEqual(result['exit_code'], 0)
            self.assertEqual((Path(directory) / 'slow.stdout.log').read_text(), 'partial')


if __name__ == '__main__':
    unittest.main()
