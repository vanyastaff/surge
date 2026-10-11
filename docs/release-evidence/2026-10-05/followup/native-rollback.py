"""Isolated terminal-only native binary replacement and full snapshot restore drill."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import tarfile
import tempfile
import time

workspace = Path('/Users/vanyastafford/Develop/surge')
old_archive = workspace / 'target/release-readiness/surge-aarch64-apple-darwin.tar.gz'
new_archive = workspace / 'target/release-candidate-settled/surge-aarch64-apple-darwin.tar.gz'
expected = {
    old_archive: '88569c9a783f26296cb31cd16c5b47f6db53c8d35a74201e2f32d9e659573e6b',
    new_archive: '7670f8e9b161609a4632b2203a3021eb976aac4dbac3fba1daa3240d4c994e4c',
}
for archive, digest in expected.items():
    assert hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest() == digest

root = Path(tempfile.mkdtemp(prefix='surge-native-rollback-', dir='/tmp'))
project, home, binaries = [root / name for name in ('project', 'home', 'bin')]
for directory in (project, home, binaries):
    directory.mkdir()
env = os.environ.copy()
for key in tuple(env):
    if key.startswith('SURGE_'):
        del env[key]
env.update(SURGE_HOME=str(home), GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null')
env['PATH'] = str(binaries) + ':/usr/bin:/bin:/usr/sbin:/sbin'
cli = binaries / 'surge'
log = []

def run(args):
    result = subprocess.run([str(x) for x in args], cwd=project, env=env,
                            capture_output=True, text=True, timeout=90)
    log.append('$ ' + ' '.join(str(x) for x in args) + '\nexit=' + str(result.returncode)
               + '\n' + result.stdout + result.stderr)
    (root / 'commands.log').write_text('\n'.join(log))
    assert result.returncode == 0, log[-1]
    return result.stdout

def stop():
    if (home / 'daemon/daemon.pid').exists():
        run([cli, 'daemon', 'stop'])
    deadline = time.monotonic() + 45
    while any((home / 'daemon' / name).exists() for name in ('daemon.pid', 'daemon.sock')):
        assert time.monotonic() < deadline, 'isolated daemon did not settle'
        time.sleep(0.1)

def install(archive, label):
    directory = root / ('extract-' + label)
    directory.mkdir()
    with tarfile.open(archive) as bundle:
        bundle.extractall(directory, filter='data')
    for name in ('surge', 'surge-daemon'):
        shutil.copy2(directory / name, binaries / name)

def inventory(parent):
    records = {}
    for path in sorted(parent.rglob('*')):
        relative = str(path.relative_to(parent))
        mode = path.lstat().st_mode & 0o777
        if path.is_symlink():
            records[relative] = ['symlink', mode, os.readlink(path)]
        elif path.is_file():
            records[relative] = ['file', mode, hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()]
        elif path.is_dir():
            records[relative] = ['directory', mode]
        else:
            raise AssertionError('unexpected snapshot entry: ' + str(path))
    return records

def databases(parent):
    # A read-only SQLite connection can create writable sidecars in its directory.
    # Validate an exact disposable copy so the retained snapshot stays immutable.
    validation = Path(tempfile.mkdtemp(prefix='db-validation-', dir=root)) / 'runtime'
    shutil.copytree(parent, validation, symlinks=True)
    assert inventory(validation) == inventory(parent)
    checked = []
    for database in sorted(validation.rglob('*.sqlite')):
        connection = sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True)
        try:
            assert connection.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
            assert connection.execute('PRAGMA foreign_key_check').fetchall() == []
        finally:
            connection.close()
        checked.append(str(database.relative_to(validation)))
    assert checked, 'no actual native runtime databases checked'
    return checked

def terminal_run():
    output = run([cli, 'engine', 'run', 'flow.toml', '--daemon', '--watch'])
    identifier = next(line for line in output.splitlines()
                      if re.fullmatch(r'(?:run-)?[0-9A-HJKMNP-TV-Z]{26}', line))
    replay = run([cli, 'engine', 'replay', identifier, '--format', 'json'])
    assert 'completed' in replay.lower()
    return identifier

report = {'root': str(root), 'old_archive_sha256': expected[old_archive],
          'new_archive_sha256': expected[new_archive], 'status': 'FAIL'}
try:
    install(old_archive, 'old')
    old_version = run([cli, '--version']).strip()
    assert '3136b82' in old_version
    run(['git', 'init'])
    (project / '.gitignore').write_text('surge.toml\n.surge/\n')
    (project / 'README.md').write_text('# Native rollback fixture\n')
    shutil.copy2(workspace / 'examples/flow_terminal_only.toml', project / 'flow.toml')
    run(['git', 'add', 'README.md', 'flow.toml', '.gitignore'])
    run(['git', '-c', 'user.name=Release drill', '-c', 'user.email=release-drill@example.invalid',
         'commit', '-m', 'Owned fixture'])
    run([cli, 'init', '--default'])
    old_runs = [terminal_run(), terminal_run()]
    stop()
    source_head = run(['git', 'rev-parse', 'HEAD'])
    assert not run(['git', 'status', '--porcelain=v1'])
    snapshot = root / 'snapshot'
    snapshot.mkdir()
    for name in ('project', 'home', 'bin'):
        shutil.copytree(root / name, snapshot / name, symlinks=True)
    before = inventory(snapshot)
    checks = databases(snapshot / 'home')
    assert inventory(snapshot) == before
    (root / 'snapshot-inventory.json').write_text(json.dumps(before, indent=2) + '\n')

    install(new_archive, 'new')
    new_version = run([cli, '--version']).strip()
    assert '8e3a781' in new_version
    run([cli, 'daemon', 'start', '--detached'])
    for identifier in old_runs:
        assert 'completed' in run([cli, 'engine', 'replay', identifier, '--format', 'json']).lower()
    new_run = terminal_run()
    stop()
    quarantine = root / 'quarantine'
    quarantine.mkdir()
    for name in ('project', 'home', 'bin'):
        (root / name).rename(quarantine / name)
        shutil.copytree(snapshot / name, root / name, symlinks=True)
    restored = {}
    for name in ('project', 'home', 'bin'):
        for path, record in inventory(root / name).items():
            restored[name + '/' + path] = record
        restored[name] = ['directory', (root / name).stat().st_mode & 0o777]
    assert restored == before, 'snapshot bytes/modes/symlinks differ after full restore'
    assert databases(home) == checks
    assert run([cli, '--version']).strip() == old_version
    assert run(['git', 'rev-parse', 'HEAD']) == source_head
    assert not run(['git', 'status', '--porcelain=v1'])
    run([cli, 'daemon', 'start', '--detached'])
    listing = run([cli, 'engine', 'ls'])
    for identifier in old_runs:
        assert identifier in listing
        assert 'completed' in run([cli, 'engine', 'replay', identifier, '--format', 'json']).lower()
    assert new_run not in listing, 'post-snapshot progress improperly overlaid during restore'
    stop()
    assert not run(['git', 'status', '--porcelain=v1'])
    assert run(['git', 'rev-parse', 'HEAD']) == source_head
    report.update(status='PASS', old_version=old_version, new_version=new_version,
                  restored_runs=old_runs, quarantined_run=new_run,
                  database_checks=checks, restored_inventory_entries=len(before),
                  quarantine=str(quarantine), limitations='Terminal-only, same current schema; no provider/MCP/external effects or production restore.')
finally:
    stop()
    (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    Path('/tmp/surge-release-native-rollback-result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
