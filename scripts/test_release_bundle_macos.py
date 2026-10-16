"""Behavioral macOS bundle tests using isolated binaries and test-only tools."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parent.parent
BINARIES = ('surge-ui', 'surge', 'surge-daemon')


class BundleTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        shutil.copyfile(SOURCE / 'scripts/bundle-macos-app.sh', self.root / 'scripts/bundle-macos-app.sh')
        shutil.copyfile(SOURCE / 'justfile', self.root / 'justfile')
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0"\n')
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.tool('uname', 'print("Darwin")')
        self.tool('codesign', 'pass')
        self.tool('git', 'print("test-sha")')
        self.tool('rustc', '''
import os, sys
if os.environ.get('MOCK_RUSTC_FAIL'):
    sys.exit(9)
print(os.environ.get('MOCK_RUSTC_TEXT', 'rustc fixture\\nhost: aarch64-apple-darwin'))
''')
        self.tool('cargo', '''
import json, os, tomllib
from pathlib import Path
import sys
root = Path.cwd()
with (root / 'cargo.jsonl').open('a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\\n')
if os.environ.get('MOCK_CARGO_FAIL'):
    sys.exit(7)
profile = 'release' if '--release' in sys.argv else 'debug'
config_path = root / '.cargo/config.toml'
config = tomllib.loads(config_path.read_text()).get('build', {}) if config_path.exists() else {}
def argument(flag, fallback):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else fallback
target_dir = Path(argument('--target-dir', os.environ.get('CARGO_TARGET_DIR', config.get('target-dir', 'target'))))
if not target_dir.is_absolute():
    target_dir = root / target_dir
target = argument('--target', os.environ.get('CARGO_BUILD_TARGET', config.get('target', '')))
for name in ('surge-ui', 'surge', 'surge-daemon'):
    path = target_dir / target / profile / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('fresh-' + profile + '-' + name)
    path.chmod(0o755)
''')
        self.env = dict(os.environ, PATH=str(self.tools) + os.pathsep + os.environ['PATH'])
        self.out = self.root / 'output'

    def tool(self, name, body):
        path = self.tools / name
        path.write_text('#!' + sys.executable + '\n' + body + '\n')
        path.chmod(0o755)

    def binaries(self, profile='debug'):
        for name in BINARIES:
            path = self.root / 'target' / profile / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('prebuilt-' + profile + '-' + name)
            path.chmod(0o755)

    def old_app(self):
        sentinel = self.out / 'Surge.app' / 'preserve-me'
        sentinel.parent.mkdir(parents=True, exist_ok=True)
        sentinel.write_text('old-bundle')
        return sentinel

    def script(self, *args, env=None):
        return subprocess.run(['bash', 'scripts/bundle-macos-app.sh', '--out', str(self.out), *args],
                              cwd=self.root, env=env or self.env, capture_output=True, text=True)

    def test_prebuilt_bundle_contains_all_three_exact_executables(self):
        self.binaries()
        result = self.script()
        self.assertEqual(result.returncode, 0, result.stderr)
        directory = self.out / 'Surge.app/Contents/MacOS'
        self.assertEqual({path.name for path in directory.iterdir()}, set(BINARIES))
        for name in BINARIES:
            self.assertEqual((directory / name).read_bytes(), (self.root / 'target/debug' / name).read_bytes())
            self.assertTrue(os.access(directory / name, os.X_OK))
        self.assertFalse((self.root / 'cargo.jsonl').exists())

    def test_each_missing_executable_preserves_existing_bundle(self):
        for name in BINARIES:
            with self.subTest(name=name):
                self.binaries()
                (self.root / 'target/debug' / name).unlink()
                sentinel = self.old_app()
                result = self.script()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(sentinel.read_text(), 'old-bundle')

    def test_non_executable_helper_preserves_existing_bundle(self):
        self.binaries()
        (self.root / 'target/debug/surge-daemon').chmod(0o644)
        sentinel = self.old_app()
        self.assertNotEqual(self.script().returncode, 0)
        self.assertTrue(sentinel.exists())

    def test_surge_home_plist_roundtrips_xml_and_spaces(self):
        self.binaries()
        home = str(self.root / 'state & <private> "quoted"')
        result = self.script('--surge-home', home)
        self.assertEqual(result.returncode, 0, result.stderr)
        with (self.out / 'Surge.app/Contents/Info.plist').open('rb') as file:
            plist = plistlib.load(file)
        self.assertEqual(plist['LSEnvironment'], {'SURGE_HOME': home})

    def test_actual_just_recipe_builds_selected_profile_and_preserves_argv(self):
        just = shutil.which('just')
        self.assertIsNotNone(just, 'just is required to verify the actual recipe')
        for profile in ('debug', 'release'):
            with self.subTest(profile=profile):
                out = self.root / ('bundle ' + profile + ' $(touch INJECTED)')
                home = str(self.root / 'home & <state> $(touch INJECTED_HOME)')
                args = ['--release'] if profile == 'release' else []
                result = subprocess.run([just, 'bundle-ui', *args, '--out', str(out), '--surge-home', home],
                                        cwd=self.root, env=self.env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = [json.loads(line) for line in (self.root / 'cargo.jsonl').read_text().splitlines()]
                self.assertEqual(len(calls), 1 if profile == 'debug' else 2)
                call = calls[-1]
                self.assertEqual(call.count('--release'), int(profile == 'release'))
                self.assertIn('--locked', call)
                self.assertIn('--target', call)
                self.assertIn('--target-dir', call)
                self.assertEqual(call[call.index('--target') + 1], 'aarch64-apple-darwin')
                self.assertEqual(Path(call[call.index('--target-dir') + 1]).resolve(),
                                 (self.root / 'target').resolve())
                self.assertEqual([call[i + 1] for i, arg in enumerate(call) if arg == '-p'],
                                 ['surge-ui', 'surge-cli', 'surge-daemon'])
                self.assertEqual([call[i + 1] for i, arg in enumerate(call) if arg == '--bin'],
                                 list(BINARIES))
                for name in BINARIES:
                    self.assertEqual((out / 'Surge.app/Contents/MacOS' / name).read_text(),
                                     'fresh-' + profile + '-' + name)
                with (out / 'Surge.app/Contents/Info.plist').open('rb') as file:
                    self.assertEqual(plistlib.load(file)['LSEnvironment']['SURGE_HOME'], home)
                self.assertFalse((self.root / 'INJECTED').exists())
                self.assertFalse((self.root / 'INJECTED_HOME').exists())

    def assert_native_build(self, env=None):
        self.binaries()
        sentinel = self.old_app()
        just = shutil.which('just')
        self.assertIsNotNone(just)
        result = subprocess.run([just, 'bundle-ui', '--out', str(self.out)], cwd=self.root,
                                env=env or self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(sentinel.exists())
        for name in BINARIES:
            packaged = self.out / 'Surge.app/Contents/MacOS' / name
            actual = self.root / 'target/aarch64-apple-darwin/debug' / name
            self.assertEqual(packaged.read_text(), 'fresh-debug-' + name)
            self.assertEqual(packaged.read_bytes(), actual.read_bytes())
            self.assertEqual((self.root / 'target/debug' / name).read_text(), 'prebuilt-debug-' + name)

    def test_build_overrides_ambient_target_directory_and_cross_target(self):
        self.assert_native_build(dict(self.env, CARGO_TARGET_DIR=str(self.root / 'elsewhere'),
                                      CARGO_BUILD_TARGET='x86_64-pc-windows-msvc'))

    def test_build_overrides_cargo_config_target_directory_and_cross_target(self):
        config = self.root / '.cargo/config.toml'
        config.parent.mkdir()
        config.write_text('[build]\ntarget-dir = "configured-cache"\ntarget = "x86_64-pc-windows-msvc"\n')
        env = dict(self.env)
        env.pop('CARGO_TARGET_DIR', None)
        env.pop('CARGO_BUILD_TARGET', None)
        self.assert_native_build(env)

    def test_invalid_or_unavailable_native_host_preserves_existing_bundle(self):
        cases = [
            {'MOCK_RUSTC_FAIL': '1'},
            {'MOCK_RUSTC_TEXT': 'rustc fixture'},
            {'MOCK_RUSTC_TEXT': 'host: aarch64-apple-darwin\nhost: aarch64-apple-darwin'},
            {'MOCK_RUSTC_TEXT': 'host: x86_64-pc-windows-msvc'},
            {'MOCK_RUSTC_TEXT': 'host: ../aarch64-apple-darwin'},
            {'MOCK_RUSTC_TEXT': 'host: aarch64-apple-darwin extra'},
            {'MOCK_RUSTC_TEXT': 'host:  aarch64-apple-darwin'},
            {'MOCK_RUSTC_TEXT': 'host: '},
        ]
        for overrides in cases:
            with self.subTest(overrides=overrides):
                self.binaries()
                sentinel = self.old_app()
                result = self.script('--build', env=dict(self.env, **overrides))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(sentinel.read_text(), 'old-bundle')
                self.assertFalse((self.root / 'cargo.jsonl').exists())

    def test_failed_build_preserves_existing_bundle(self):
        self.binaries()
        sentinel = self.old_app()
        result = self.script('--build', env=dict(self.env, MOCK_CARGO_FAIL='1'))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(sentinel.read_text(), 'old-bundle')
        calls = [json.loads(line) for line in (self.root / 'cargo.jsonl').read_text().splitlines()]
        self.assertEqual(len(calls), 1)


if __name__ == '__main__':
    unittest.main()
