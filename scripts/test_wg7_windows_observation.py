"""Fixed observation receipt checks, independent of Windows test execution."""
import copy
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

import wg7_windows_observation as observation


def inventory():
    suites = {}
    for binary, name in observation.CASES:
        suite = suites.setdefault(binary, {'binary-id': binary, 'binary-path': binary + '.exe', 'testcases': {}})
        suite['testcases'][name] = {'ignored': False, 'filter-match': {'status': 'matches'}}
    return {'rust-suites': suites}


class ObservationTests(unittest.TestCase):
    def test_inventory_requires_exact_six_nonignored_selected_identities(self):
        original = inventory()
        self.assertEqual(len(observation.CASES), 6)
        self.assertEqual(observation.inventory_errors(original)[0], [])
        original_five = copy.deepcopy(original)
        del original_five['rust-suites']['surge-daemon::owned_flow_mcp_recovery']['testcases'][
            'populated_cold_valid_twin_keeps_original_transport_and_writer_history']
        self.assertTrue(observation.inventory_errors(original_five)[0])
        for kind in ('zero', 'missing', 'extra', 'ignored', 'wrong-binary', 'duplicate'):
            with self.subTest(kind=kind):
                value = copy.deepcopy(original)
                binary, name = observation.CASES[0]
                suite = value['rust-suites'][binary]
                if kind == 'zero':
                    value['rust-suites'] = {}
                elif kind == 'missing':
                    del suite['testcases'][name]
                elif kind == 'extra':
                    suite['testcases']['unexpected'] = copy.deepcopy(suite['testcases'][name])
                elif kind == 'ignored':
                    suite['testcases'][name]['ignored'] = True
                elif kind == 'wrong-binary':
                    suite['binary-id'] = 'other'
                else:
                    value['rust-suites']['copy'] = copy.deepcopy(suite)
                self.assertTrue(observation.inventory_errors(value)[0])

    def test_unselected_inventory_entries_do_not_count_as_executions(self):
        value = inventory()
        value['rust-suites']['other'] = {'binary-id': 'other', 'binary-path': 'other.exe',
            'testcases': {'unselected': {'ignored': True, 'filter-match': {'status': 'mismatch'}}}}
        errors, binaries = observation.inventory_errors(value)
        self.assertEqual(errors, [])
        self.assertNotIn('other', binaries)

    def check_xml(self, mutation=None):
        root = ET.Element('testsuites', tests='999', failures='0')
        for binary, name in observation.CASES:
            suite = ET.SubElement(root, 'testsuite', name=binary)
            ET.SubElement(suite, 'testcase', name=name, classname=binary)
        if mutation:
            mutation(root)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'actual.xml'
            ET.ElementTree(root).write(path)
            return observation.junit_errors(path)

    def test_junit_counts_actual_nodes_and_exact_binary_identity(self):
        errors, summary = self.check_xml()
        self.assertEqual(errors, [])
        self.assertEqual(summary['total'], 6)
        self.assertEqual(summary['passed'], 6)
        for tag in ('skipped', 'failure', 'error'):
            with self.subTest(tag=tag):
                errors, _ = self.check_xml(lambda root: ET.SubElement(root[0][0], tag))
                self.assertTrue(errors)
        for field in ('name', 'classname'):
            with self.subTest(field=field):
                errors, _ = self.check_xml(lambda root: root[0][0].set(field, 'wrong'))
                self.assertTrue(errors)
        for kind in ('missing', 'duplicate', 'zero'):
            with self.subTest(kind=kind):
                def mutate(root):
                    if kind == 'zero':
                        root.clear()
                    elif kind == 'missing':
                        root.remove(root[0])
                    else:
                        root.append(copy.deepcopy(root[0]))
                self.assertTrue(self.check_xml(mutate)[0])

    def test_missing_or_malformed_junit_is_not_success(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'missing.xml'
            with self.assertRaises(OSError):
                observation.junit_errors(path)
            path.write_text('<not-xml', encoding='utf-8')
            with self.assertRaises(ET.ParseError):
                observation.junit_errors(path)

    def test_revision_dirty_sha_and_binary_drift_refuse(self):
        before = {'revision': 'fixed', 'github_sha': 'fixed', 'dirty': '',
                  'binaries': {'test': {'path': 'fixed.exe', 'sha256': 'abc', 'bytes': 4}}}
        self.assertEqual(observation.identity_errors(before, copy.deepcopy(before)), [])
        for field, value in (('revision', 'changed'), ('dirty', ' M source.rs'),
                             ('binaries', {})):
            with self.subTest(field=field):
                after = copy.deepcopy(before)
                after[field] = value
                self.assertTrue(observation.identity_errors(before, after))
        before['github_sha'] = 'other'
        self.assertTrue(observation.identity_errors(before, before))

    def test_outcomes_fail_even_if_continue_on_error_masks_conclusion(self):
        success = dict.fromkeys(observation.OUTCOMES, 'success')
        self.assertEqual(observation.outcome_errors(success), [])
        for key in observation.OUTCOMES:
            for outcome in ('failure', 'skipped', 'cancelled', ''):
                with self.subTest(key=key, outcome=outcome):
                    actual = dict(success, **{key: outcome})
                    self.assertTrue(observation.outcome_errors(actual))
        self.assertTrue(observation.outcome_errors({}))
        self.assertTrue(observation.outcome_errors(dict(success, conclusion='success')))

    def test_file_identity_detects_content_change_and_refuses_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'binary.exe'
            path.write_bytes(b'first')
            first = observation.file_identity(path)
            path.write_bytes(b'other')
            self.assertNotEqual(first['sha256'], observation.file_identity(path)['sha256'])
            link = Path(directory) / 'link.exe'
            try:
                link.symlink_to(path)
            except OSError:
                return  # Windows standard users may lack symlink creation privileges.
            with self.assertRaises(ValueError):
                observation.file_identity(link)


if __name__ == '__main__':
    unittest.main()
