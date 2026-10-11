"""Independent fixed fixtures for release-native fail-closed evidence checks."""
import importlib.util
import json
import hashlib
import os
from pathlib import Path
import tempfile
import types
import sys
import io
import tarfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release_native', Path(__file__).with_name('release_native.py'))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class NativeTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def file(self, name, content='evidence'):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    def messages(self, values):
        return self.file('build.jsonl', '\n'.join(json.dumps(v) for v in values))

    def test_rejects_failed_missing_duplicate_and_trailing_success(self):
        cases = [[], [{'reason': 'build-finished', 'success': False}],
                 [{'reason': 'build-finished', 'success': True}] * 2,
                 [{'reason': 'build-finished', 'success': True}, {'reason': 'compiler-artifact'}]]
        for values in cases:
            with self.subTest(values=values), self.assertRaises(ValueError):
                native.read_messages(self.messages(values))

    def test_binary_set_requires_two_non_test_artifacts(self):
        cli = self.file('surge')
        daemon = self.file('surge-daemon')
        artifacts = [dict(reason='compiler-artifact', executable=str(path),
                          target={'name': name}, profile={'test': False})
                     for name, path in [('surge', cli), ('surge-daemon', daemon)]]
        self.assertEqual(set(native.binary_artifacts(artifacts)), {'surge', 'surge-daemon'})
        for invalid in [artifacts[:1], artifacts + artifacts[:1],
                        [dict(artifacts[0], profile={'test': True}), artifacts[1]]]:
            with self.assertRaises(ValueError):
                native.binary_artifacts(invalid)

    def component(self, name='libgit2-sys', library='git2', cfg=True):
        out = self.root / 'build/selected/out'
        archive = self.file(f'build/selected/out/lib{library}.a', 'fixed native archive')
        self.file('build/selected/output', f'cargo:rustc-link-lib=static={library}\n')
        manifest = self.file('source/Cargo.toml')
        self.file('source/build.rs', 'reviewed build source')
        message = dict(out_dir=str(out), linked_libs=[f'static={library}'],
                       linked_paths=[f'native={out}'], cfgs=['libgit2_vendored'] if cfg else [])
        package = dict(id='fixed-id', name=name, version='1.0.0', manifest_path=str(manifest), source='registry+fixed')
        return message, package, archive

    def test_vendor_archive_is_bound_to_current_event_and_out_directory(self):
        m, p, archive = self.component()
        result = native.native_component(m, p)
        self.assertEqual(result['archives'][0]['sha256'], native.digest(archive))
        self.assertEqual(result['origin']['kind'], 'crate-vendor')
        m['linked_libs'] = ['static=another']
        with self.assertRaisesRegex(ValueError, 'event/output'):
            native.native_component(m, p)

    def test_unknown_static_recipe_and_nonvendor_git2_fail(self):
        m, p, _ = self.component(name='unknown-sys')
        with self.assertRaisesRegex(ValueError, 'Unknown static'):
            native.native_component(m, p)
        p['name'] = 'libgit2-sys'
        m['cfgs'] = []
        with self.assertRaisesRegex(ValueError, 'vendored cfg'):
            native.native_component(m, p)

    def test_external_and_ambiguous_archive_paths_fail(self):
        m, p, _ = self.component()
        external = self.file('external/libgit2.a')
        m['linked_paths'] = [f'native={external.parent}']
        with self.assertRaisesRegex(ValueError, 'outside vendor'):
            native.native_component(m, p)
        m['linked_paths'].append(f'native={self.root / "build/selected/out"}')
        with self.assertRaisesRegex(ValueError, '2 resolved'):
            native.native_component(m, p)

    def test_tampered_evidence_is_rejected(self):
        path = self.file('proof')
        record = native.evidence(path)
        path.write_text('changed')
        with self.assertRaisesRegex(ValueError, 'Evidence changed'):
            native.verify_evidence(record)

    def test_duplicate_json_keys_are_rejected_in_nested_evidence(self):
        for text in ('{"success": false, "success": true}',
                     '{"binary": {"sha256": "old", "sha256": "new"}}'):
            with self.assertRaisesRegex(ValueError, 'Duplicate JSON key'):
                native.strict_json(text)

    def test_openssl_uses_same_package_root_and_header(self):
        root = self.root / 'openssl/3.6.4'
        ssl = self.file('openssl/3.6.4/lib/libssl.a')
        crypto = self.file('openssl/3.6.4/lib/libcrypto.a')
        self.file('openssl/3.6.4/INSTALL_RECEIPT.json', json.dumps({'source': {'versions': {'stable': '3.6.4'}}}))
        self.file('openssl/3.6.4/LICENSE.txt', 'authentic package license fixture')
        header = self.file('openssl/3.6.4/include/openssl/opensslv.h', '#define OPENSSL_VERSION_STR "3.6.4"')
        output = f'version: 3_6_4\ncargo:include={root}/include\n'
        origin = native.openssl_origin([], [ssl, crypto], output)
        self.assertEqual(origin['version'], '3.6.4')
        header.write_text('#define OPENSSL_VERSION_STR "9.0.0"')
        with self.assertRaisesRegex(ValueError, 'header version'):
            native.openssl_origin([], [ssl, crypto], output)

    def test_macos_loader_rejects_homebrew_dependencies(self):
        good = 'surge:\n\t/usr/lib/libSystem.B.dylib (version)\n'
        with patch.object(native, 'run', return_value=good):
            self.assertEqual(native.loader(Path('surge'), 'aarch64-apple-darwin')['libraries'], ['/usr/lib/libSystem.B.dylib'])
        with patch.object(native, 'run', return_value=good + '\t/opt/homebrew/libssl.dylib (version)\n'):
            with self.assertRaisesRegex(ValueError, 'Unbundled'):
                native.loader(Path('surge'), 'aarch64-apple-darwin')

    def test_runtime_requires_authentic_document_and_target_archives(self):
        chain = {'sysroot': str(self.root), 'version': 'rustc fixed commit'}
        self.file('share/doc/rust/COPYRIGHT-library.html', 'Copyright notices for The Rust Standard Library\nLicense body')
        self.file('lib/rustlib/fixed-target/lib/libstd-fixed.rlib')
        value = native.runtime(chain, 'fixed-target')
        self.assertIn('License body', value['copyright']['text'])
        with self.assertRaisesRegex(ValueError, 'archives missing'):
            native.runtime(chain, 'missing-target')

    def test_exact_graph_rejects_omitted_pure_rust_dependency(self):
        import release_notices
        packages = [dict(id=name, name=name, version='1', source=None, manifest_path=str(self.root / name / 'Cargo.toml'))
                    for name in ('surge-cli', 'surge-daemon', 'pure-rust')]
        metadata = self.file('metadata.json', json.dumps({'packages': packages}))
        ids = self.file('ids.json', json.dumps(['surge-cli', 'surge-daemon']))
        with patch.object(release_notices, 'production_graph', return_value=([(p, '') for p in packages], ['cargo', 'tree'], {})):
            with self.assertRaisesRegex(ValueError, 'actual locked production graph'):
                native.verify_graph(metadata, ids, 'fixed')

    def test_snapshot_workspace_identity_changes_with_notice_source(self):
        manifest = self.file('source/Cargo.toml', 'manifest')
        notice_script = self.file('scripts/release_notices.py', 'before')
        metadata = self.file('metadata.json', json.dumps({'packages': [dict(id='fixed', manifest_path=str(manifest))]}))
        ids = self.file('ids.json', '["fixed"]')
        git = types.SimpleNamespace(stdout=(str(notice_script) + '\0').encode())
        with patch.object(native.subprocess, 'run', return_value=git):
            before = native.snapshot_sources(metadata, ids)['workspace_identity']
            notice_script.write_text('after')
            after = native.snapshot_sources(metadata, ids)['workspace_identity']
        self.assertNotEqual(before['sha256'], after['sha256'])
        self.assertEqual(before['files'], [[str(notice_script), hashlib.sha256(b'before').hexdigest()]])
        self.assertEqual(after['files'], [[str(notice_script), hashlib.sha256(b'after').hexdigest()]])

    def test_registry_authentication_retains_nested_target_and_rejects_mutation(self):
        crate = 'native-sys-1.0.0'
        source = self.root / 'registry/src/fixed-index' / crate
        source.mkdir(parents=True)
        archive = self.root / 'registry/cache/fixed-index' / (crate + '.crate')
        archive.parent.mkdir(parents=True)
        published = {'Cargo.toml': b'[package]\nname="native-sys"\nversion="1.0.0"\n',
                     'src/target/actual.c': b'original compiled source\n'}
        with tarfile.open(archive, 'w:gz') as stream:
            for relative, content in published.items():
                member = tarfile.TarInfo(crate + '/' + relative)
                member.size = len(content)
                stream.addfile(member, io.BytesIO(content))
                path = source / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
        package = dict(id='registry-fixed', name='native-sys', version='1.0.0',
                       source='registry+https://fixed.example/index', manifest_path=str(source / 'Cargo.toml'))
        locked = [dict(name=package['name'], version=package['version'], source=package['source'],
                       checksum=hashlib.sha256(archive.read_bytes()).hexdigest())]
        authenticated = native.authenticate_registry_source(package, locked)
        self.assertEqual(authenticated['published_files']['src/target/actual.c'], hashlib.sha256(published['src/target/actual.c']).hexdigest())
        metadata = self.file('metadata.json', json.dumps({'packages': [package]}))
        ids = self.file('ids.json', '["registry-fixed"]')
        self.file('Cargo.lock', 'version=4\n[[package]]\nname="native-sys"\nversion="1.0.0"\nsource="registry+https://fixed.example/index"\nchecksum="' + locked[0]['checksum'] + '"\n')
        previous = Path.cwd()
        os.chdir(self.root)
        try:
            with patch.object(native.subprocess, 'run', return_value=types.SimpleNamespace(stdout=b'')):
                snapshot = native.snapshot_sources(metadata, ids)
            self.assertIn(str((source / 'src/target/actual.c').resolve()), [p['path'] for p in snapshot['packages'][package['id']]])
        finally:
            os.chdir(previous)
        (source / 'src/target/actual.c').write_bytes(b'mutated native source\n')
        self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), locked[0]['checksum'])
        with self.assertRaisesRegex(ValueError, 'source differs from authenticated archive'):
            native.authenticate_registry_source(package, locked)

    def test_producer_collector_integration_binds_strip_and_keeps_native_gaps_open(self):
        import release_notices
        target = 'aarch64-apple-darwin'
        m, vendor, _ = self.component()
        # Self-contained authenticated registry fixture: CI does not need an ambient Cargo cache.
        published = {'Cargo.toml': b'[package]\nname="libgit2-sys"\nversion="1.0.0"\n',
                     'build.rs': b'reviewed build source'}
        registry_source = self.root / 'registry/src/fixed-index/libgit2-sys-1.0.0'
        registry_source.mkdir(parents=True)
        archive = self.root / 'registry/cache/fixed-index/libgit2-sys-1.0.0.crate'
        archive.parent.mkdir(parents=True)
        with tarfile.open(archive, 'w:gz') as stream:
            for relative, content in published.items():
                member = tarfile.TarInfo('libgit2-sys-1.0.0/' + relative)
                member.size = len(content)
                stream.addfile(member, io.BytesIO(content))
                (registry_source / relative).write_bytes(content)
        archive_hash = hashlib.sha256(archive.read_bytes()).hexdigest()
        vendor['manifest_path'] = str((registry_source / 'Cargo.toml').resolve())
        packages = [vendor]
        for name in ('surge-cli', 'surge-daemon'):
            manifest = self.file(f'{name}/Cargo.toml', 'source manifest')
            packages.append(dict(id=name, name=name, version='1', source=None, manifest_path=str(manifest)))
        metadata = self.file('metadata.json', json.dumps({'packages': packages}))
        ids = self.file('ids.json', json.dumps([p['id'] for p in packages]))
        cli = self.file('bin/surge', 'before-cli')
        daemon = self.file('bin/surge-daemon', 'before-daemon')
        events = [dict(reason='compiler-artifact', package_id=key, executable=str(path),
                       target={'name': name}, profile={'test': False})
                  for key, name, path in [('surge-cli', 'surge', cli), ('surge-daemon', 'surge-daemon', daemon)]]
        events.append(dict(m, reason='build-script-executed', package_id=vendor['id']))
        events.append(dict(reason='build-finished', success=True))
        messages = self.messages(events)
        self.file('Cargo.toml', 'workspace fixture')
        lock_text = 'version = 4\n\n[[package]]\nname = "libgit2-sys"\nversion = "1.0.0"\nsource="registry+fixed"\nchecksum="' + archive_hash + '"\n'
        lock = self.file('Cargo.lock', lock_text)
        runtime_root = self.root / 'runtime'
        self.file('runtime/share/doc/rust/COPYRIGHT-library.html', 'Copyright notices for The Rust Standard Library\nExact fixture terms')
        self.file(f'runtime/lib/rustlib/{target}/lib/libstd-fixed.rlib', 'fixed-stdlib')
        chain = {'version': f'rustc fixture\nhost: {target}\ncommit-hash: fixed', 'sysroot': str(runtime_root)}
        old = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, old)
        def execute(command, **kwargs):
            if command[0] == 'git':
                return types.SimpleNamespace(stdout=b'Cargo.toml\0Cargo.lock\0')
            if command[0] == str(Path(sys.executable).resolve()):
                Path(command[1]).write_text('stripped-' + Path(command[1]).read_text())
                return types.SimpleNamespace(returncode=0)
            self.fail(f'Unexpected subprocess: {command}')
        def tool_output(command):
            if command[0] == 'which':
                return sys.executable + '\n'
            if command[0] == 'otool':
                return 'binary:\n\t/usr/lib/libSystem.B.dylib (version)\n'
            self.fail(f'Unexpected tool: {command}')
        graph = ([(p, '') for p in packages], ['cargo', 'tree'], {})
        with patch.object(release_notices, 'production_graph', return_value=graph), patch.object(native, 'toolchain', return_value=chain), patch.object(native.subprocess, 'run', side_effect=execute), patch.object(native, 'run', side_effect=tool_output):
            capture = dict(schema=1, target=target, toolchain=chain,
                           build_messages=native.evidence(messages), cargo_lock=native.evidence(lock),
                           metadata=native.evidence(metadata), dependency_ids=native.evidence(ids),
                           source_inputs=native.snapshot_sources(metadata, ids),
                           runtime_inputs=native.runtime(chain, target), production_graph=native.verify_graph(metadata, ids, target),
                           binaries=[dict(name=name, **native.evidence(path)) for name, path in [('surge', cli), ('surge-daemon', daemon)]])
            Path(str(messages) + '.capture.json').write_text(json.dumps(capture))
            output = self.root / 'receipt.json'
            native.produce(types.SimpleNamespace(target=target, metadata=metadata, dependency_ids=ids,
                           build_messages=messages, bin_dir=cli.parent, output=output, strip_tool='fixture-strip'))
        record = json.loads(output.read_text())
        self.assertTrue(record['complete'])
        self.assertEqual(record['binaries'][0]['before']['sha256'], hashlib.sha256(b'before-cli').hexdigest())
        self.assertEqual(record['binaries'][0]['after']['sha256'], hashlib.sha256(b'stripped-before-cli').hexdigest())
        registry_rows = [{'id': vendor['id'], 'name': vendor['name'], 'version': vendor['version'],
                          'crate_sha256': archive_hash,
                          'published_files': {p: hashlib.sha256(b).hexdigest() for p, b in published.items()},
                          'notices': []}]
        accepted, gaps = release_notices.native_coverage(output, target, hashlib.sha256(lock_text.encode()).hexdigest(), set(p['id'] for p in packages), registry_rows)
        self.assertEqual(accepted['receipt']['binaries'][0]['transformation']['kind'], 'strip')
        self.assertTrue(any('source/header notice mapping' in g for g in gaps))


if __name__ == '__main__':
    unittest.main()
