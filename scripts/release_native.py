"""Capture native release inputs; unknown origins remain explicit completeness blockers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

VENDOR_LIBRARIES = {
    'libgit2-sys': {'git2'}, 'libssh2-sys': {'ssh2'},
    'libz-sys': {'z', 'zlib', 'zlibstatic'}, 'libsqlite3-sys': {'sqlite3'},
    'ring': {'ring_core_0_17_14_', 'ring_core_0_17_14__test'},
    'psm': {'psm_s'}, 'mac-notification-sys': {'notify'},
}


def strict_json(text):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError(f'Duplicate JSON key: {key}')
            result[key] = value
        return result
    return json.loads(text, object_pairs_hook=pairs)


def digest(path):
    path = Path(path)
    if not path.is_file() or path.stat().st_size == 0:
        raise ValueError(f'Missing or empty evidence file: {path}')
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def evidence(path, text=False):
    path = Path(path).resolve(strict=True)
    value = {'path': str(path), 'sha256': digest(path)}
    if text:
        value['text'] = path.read_text(encoding='utf-8')
    return value


def run(command):
    result = subprocess.run(command, capture_output=True, text=True, check=True)
    return result.stdout


def toolchain():
    version = run(['rustc', '-vV'])
    sysroot = Path(run(['rustc', '--print', 'sysroot']).strip()).resolve(strict=True)
    return {'version': version, 'sysroot': str(sysroot)}


def read_messages(path):
    messages = []
    for line in Path(path).read_text().splitlines():
        message = strict_json(line)
        if not isinstance(message, dict) or 'reason' not in message:
            raise ValueError('Invalid Cargo JSON message')
        messages.append(message)
    finished = [m for m in messages if m['reason'] == 'build-finished']
    if len(finished) != 1 or finished[0].get('success') is not True or messages[-1] != finished[0]:
        raise ValueError('Cargo stream does not end in one successful build-finished')
    return messages


def binary_artifacts(messages):
    artifacts = {}
    for m in messages:
        if m['reason'] != 'compiler-artifact' or not m.get('executable'):
            continue
        name = m.get('target', {}).get('name')
        if name not in {'surge', 'surge-daemon'}:
            continue
        if m.get('profile', {}).get('test') or name in artifacts:
            raise ValueError(f'Duplicate or test binary artifact: {name}')
        artifacts[name] = Path(m['executable']).resolve(strict=True)
    if set(artifacts) != {'surge', 'surge-daemon'}:
        raise ValueError('Cargo stream must contain both release binary artifacts')
    return artifacts


def capture_build(args):
    for key in ('RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER'):
        if os.environ.get(key):
            raise ValueError(f'Unproven compiler override: {key}')
    chain = toolchain()
    graph_proof = verify_graph(args.metadata, args.dependency_ids, args.target)
    host = re.search(r'^host: (.+)$', chain['version'], re.M)
    if args.native_host and (not host or host.group(1) != args.target):
        raise ValueError('--native-host requires requested target to equal rustc host')
    source_inputs = snapshot_sources(args.metadata, args.dependency_ids)
    runtime_inputs = runtime(chain, args.target)
    output = args.output.resolve()
    if output.is_relative_to(Path.cwd().resolve()):
        ignored = subprocess.run(['git', 'check-ignore', '--quiet', str(output)], check=False)
        if ignored.returncode:
            raise ValueError('Build proof inside workspace must be Git-ignored (use target/ or /tmp)')
    output.parent.mkdir(parents=True, exist_ok=True)
    command = ['cargo', 'build', '--locked', '--release', '-p', 'surge-cli', '-p', 'surge-daemon', '--message-format=json']
    if not args.native_host:
        command += ['--target', args.target]
    with output.open('w') as stream:
        result = subprocess.run(command, stdout=stream)
    if result.returncode:
        raise ValueError(f'Cargo build failed with exit code {result.returncode}')
    messages = read_messages(output)
    if source_inputs != snapshot_sources(args.metadata, args.dependency_ids):
        raise ValueError('Production source inputs changed during build')
    if runtime_inputs != runtime(chain, args.target):
        raise ValueError('Rust runtime inputs changed during build')
    capture = {
        'schema': 1, 'target': args.target, 'command': command,
        'build_messages': evidence(output), 'cargo_lock': evidence('Cargo.lock'),
        'toolchain': chain,
        'metadata': evidence(args.metadata), 'dependency_ids': evidence(args.dependency_ids),
        'source_inputs': source_inputs,
        'runtime_inputs': runtime_inputs,
        'production_graph': graph_proof,
        'binaries': [{'name': name, **evidence(path)} for name, path in binary_artifacts(messages).items()],
    }
    Path(str(output) + '.capture.json').write_text(json.dumps(capture, indent=2) + '\n')


def verify_graph(metadata_path, ids_path, target):
    # Reuse the collector's exact normal/build graph contract, not a guessed workspace set.
    from release_notices import production_graph
    selected, command, _ = production_graph(target)
    ids = strict_json(ids_path.read_text())
    expected = {p['id']: p for p, _ in selected}
    supplied = {p['id']: p for p in strict_json(metadata_path.read_text())['packages']}
    if not isinstance(ids, list) or set(ids) != set(expected) or len(ids) != len(expected):
        raise ValueError('Supplied dependency IDs differ from actual locked production graph')
    for key, package in expected.items():
        if key not in supplied or any(supplied[key].get(field) != package.get(field)
                                     for field in ('id', 'name', 'version', 'source', 'manifest_path')):
            raise ValueError('Supplied metadata differs from actual production source identity')
    return {'command': command, 'package_ids': sorted(expected)}


def snapshot_sources(metadata_path, ids_path):
    packages = {p['id']: p for p in strict_json(metadata_path.read_text())['packages']}
    ids = strict_json(ids_path.read_text())
    if not isinstance(ids, list) or not ids or len(set(ids)) != len(ids) or not set(ids) <= packages.keys():
        raise ValueError('Dependency IDs must be a nonempty unique list of metadata package IDs')
    files = {}
    for package_id in sorted(ids):
        source = Path(packages[package_id]['manifest_path']).parent.resolve(strict=True)
        package_files = []
        for base, dirs, names in os.walk(source, followlinks=False):
            dirs[:] = sorted(d for d in dirs if d not in {'.git', 'target', '.worktrees', '__pycache__'})
            if any((Path(base) / d).is_symlink() for d in dirs):
                raise ValueError(f'Unproven source directory symlink in {base}')
            for name in sorted(names):
                path = Path(base) / name
                if path.is_symlink():
                    raise ValueError(f'Unproven source symlink: {path}')
                package_files.append({'path': str(path.resolve()), 'sha256': digest(path) if path.stat().st_size else hashlib.sha256(b'').hexdigest()})
        files[package_id] = package_files
    # Workspace resolution and Cargo configuration also influence trusted Cargo builds.
    context = []
    for path in (Path('Cargo.toml'), Path('Cargo.lock'), Path('.cargo/config.toml'), Path('.cargo/config'), Path('rust-toolchain.toml'), Path('rust-toolchain')):
        if path.is_file():
            context.append(evidence(path))
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    for path in (cargo_home / 'config', cargo_home / 'config.toml'):
        if path.is_file():
            context.append(evidence(path))
    # Include workspace files outside crate directories used by include_str!/build.rs.
    tracked = subprocess.run(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'],
                             capture_output=True, check=True).stdout.decode().split('\0')
    workspace = []
    for raw in sorted(set(tracked) - {''}):
        path = Path(raw)
        if path.is_symlink():
            raise ValueError(f'Unproven workspace source symlink: {path}')
        if path.is_file():
            workspace.append({'path': str(path.resolve()),
                              'sha256': digest(path) if path.stat().st_size else hashlib.sha256(b'').hexdigest()})
    return {'packages': files, 'workspace_context': context, 'workspace_files': workspace}


def verify_evidence(record):
    if digest(record['path']) != record['sha256']:
        raise ValueError(f'Evidence changed: {record["path"]}')


def archive_for(library, paths):
    candidates = []
    for raw in paths:
        path = Path(raw.split('=', 1)[1] if raw.startswith('native=') else raw)
        for filename in (f'lib{library}.a', f'{library}.lib', f'lib{library}.lib'):
            candidate = path / filename
            if candidate.is_file():
                candidates.append(candidate.resolve())
    candidates = list(dict.fromkeys(candidates))
    if len(candidates) != 1:
        raise ValueError(f'Static library {library} has {len(candidates)} resolved archive candidates')
    return candidates[0]


def openssl_origin(paths, archives, output):
    roots = {p.parent.parent.resolve() for p in archives}
    if len(roots) != 1:
        raise ValueError('OpenSSL archives do not share one package root')
    root = roots.pop()
    receipt = root / 'INSTALL_RECEIPT.json'
    data = strict_json(receipt.read_text())
    version = data['source']['versions']['stable']
    header = root / 'include/openssl/opensslv.h'
    header_text = header.read_text()
    if f'"{version}"' not in header_text and f'OpenSSL {version} ' not in header_text:
        raise ValueError('OpenSSL header version differs from package receipt')
    match = re.search(r'^version: (\d+)_(\d+)_(\d+)$', output, re.M)
    if not match or '.'.join(match.groups()) != version:
        raise ValueError('OpenSSL emitted version differs from package receipt')
    include = re.findall(r'^cargo:include=(.+)$', output, re.M)
    if len(include) != 1 or Path(include[0]).resolve() != root / 'include':
        raise ValueError('OpenSSL include root differs from linked archive root')
    return {'kind': 'system-package', 'root': str(root), 'version': version,
            'proof_files': [evidence(receipt), evidence(header)],
            'licenses': [evidence(root / 'LICENSE.txt', text=True)]}


def native_component(message, package):
    out = Path(message['out_dir']).resolve(strict=True)
    output_path = out.parent / 'output'
    output = output_path.read_text()
    # Cargo's current event and persisted output must agree; never choose stale output by glob.
    emitted = re.findall(r'^cargo(?::|::)rustc-link-lib=(.+)$', output, re.M)
    if emitted != message.get('linked_libs', []):
        raise ValueError(f'Cargo event/output link mismatch for {package["name"]}')
    libs = message.get('linked_libs', [])
    paths = message.get('linked_paths', [])
    static = [lib.split('=', 1)[1] for lib in libs if lib.startswith('static=')]
    record = {'package_id': package['id'], 'package_version': package['version'],
              'build_output': evidence(output_path), 'out_dir': str(out),
              'linked_libs': libs, 'linked_paths': paths, 'cfgs': message.get('cfgs', []),
              'archives': []}
    if not static:
        record['origin'] = {'kind': 'system-dynamic', 'verification': 'final-loader'}
        return record
    archives = [archive_for(lib, paths) for lib in static]
    record['archives'] = [{'library': lib, **evidence(path)} for lib, path in zip(static, archives)]
    if package['name'] == 'openssl-sys':
        record['origin'] = openssl_origin(paths, archives, output)
        return record
    if package['name'] not in VENDOR_LIBRARIES or not set(static) <= VENDOR_LIBRARIES[package['name']]:
        raise ValueError(f'Unknown static native recipe: {package["name"]}: {static}')
    if any(not p.is_relative_to(out) for p in archives):
        raise ValueError(f'Native archive outside vendor output: {package["name"]}')
    if package['name'] == 'libgit2-sys' and 'libgit2_vendored' not in message.get('cfgs', []):
        raise ValueError('libgit2 static archive lacks explicit vendored cfg')
    source = Path(package['manifest_path']).parent
    record['origin'] = {'kind': 'crate-vendor', 'root': str(source),
                        'build_recipe': evidence(source / 'build.rs'),
                        'source': package.get('source'), 'version': package['version']}
    return record


def loader(binary, target):
    if 'apple-darwin' in target:
        command = ['otool', '-L', str(binary)]
        output = run(command)
        libraries = [line.split()[0] for line in output.splitlines()[1:]]
        if any(not p.startswith(('/usr/lib/', '/System/Library/')) for p in libraries):
            raise ValueError(f'Unbundled macOS runtime dependency in {binary}')
    elif 'linux' in target:
        command = ['readelf', '-d', str(binary)]
        output = run(command)
        libraries = re.findall(r'\(NEEDED\).*\[(.+?)\]', output)
    else:
        command = ['dumpbin', '/DEPENDENTS', str(binary)]
        output = run(command)
        libraries = re.findall(r'^\s+([\w.-]+\.dll)\s*$', output, re.M | re.I)
    if not libraries:
        raise ValueError(f'No loader dependencies found for {binary}')
    return {'command': command, 'output': output, 'libraries': libraries}


def runtime(chain, target):
    root = Path(chain['sysroot'])
    notice = root / 'share/doc/rust/COPYRIGHT-library.html'
    document = evidence(notice, text=True)
    if 'Copyright notices for The Rust Standard Library' not in document['text']:
        raise ValueError('Unrecognized Rust runtime copyright document')
    libraries = sorted((root / 'lib/rustlib' / target / 'lib').glob('*.rlib'))
    if not libraries:
        raise ValueError('Rust target sysroot archives missing')
    return {'toolchain': chain, 'copyright': document,
            'archives': [evidence(path) for path in libraries],
            'scope': 'Conservative full target std/sysroot inventory, not exact linker-selected subset'}


def produce(args):
    capture = strict_json(Path(str(args.build_messages) + '.capture.json').read_text())
    if capture.get('schema') != 1 or capture.get('target') != args.target:
        raise ValueError('Build capture target/schema mismatch')
    verify_evidence(capture['build_messages'])
    if Path(capture['build_messages']['path']).resolve() != args.build_messages.resolve():
        raise ValueError('Build capture belongs to another JSON stream')
    verify_evidence(capture['cargo_lock'])
    if Path(capture['cargo_lock']['path']).resolve() != Path('Cargo.lock').resolve():
        raise ValueError('Build capture belongs to another workspace lock')
    if capture['toolchain'] != toolchain():
        raise ValueError('Rust toolchain changed after build')
    if verify_graph(args.metadata, args.dependency_ids, args.target) != capture['production_graph']:
        raise ValueError('Production graph changed after build')
    for key, path in (('metadata', args.metadata), ('dependency_ids', args.dependency_ids)):
        verify_evidence(capture[key])
        if Path(capture[key]['path']).resolve() != path.resolve():
            raise ValueError(f'Build capture {key} file differs from producer input')
    if snapshot_sources(args.metadata, args.dependency_ids) != capture['source_inputs']:
        raise ValueError('Production source inputs changed after build')
    if runtime(capture['toolchain'], args.target) != capture['runtime_inputs']:
        raise ValueError('Rust runtime inputs changed after build')
    messages = read_messages(args.build_messages)
    artifacts = binary_artifacts(messages)
    packages = {p['id']: p for p in strict_json(args.metadata.read_text())['packages']}
    ids = strict_json(args.dependency_ids.read_text())
    if not isinstance(ids, list) or not ids or len(set(ids)) != len(ids) or not set(ids) <= packages.keys():
        raise ValueError('Dependency IDs must be a unique list of metadata package IDs')
    unmatched = {m['package_id'] for m in messages if m['reason'] in
                 {'compiler-artifact', 'build-script-executed'} and m.get('package_id') not in ids}
    if unmatched:
        raise ValueError(f'Build events outside selected production graph: {sorted(unmatched)}')
    receipt = {'schema': 1, 'target': args.target, 'complete': False, 'blockers': [],
               'cargo_lock': capture['cargo_lock'], 'build_messages': capture['build_messages'],
               'metadata': evidence(args.metadata), 'dependency_ids': evidence(args.dependency_ids),
               'source_inputs': capture['source_inputs'],
               'production_graph': capture['production_graph'],
               'components': [], 'binaries': [], 'runtime': None}
    seen = set()
    for m in messages:
        if m['reason'] != 'build-script-executed' or not m.get('linked_libs'):
            continue
        package_id = m['package_id']
        if package_id not in ids:
            raise ValueError(f'Unmatched native build event: {package_id}')
        if package_id in seen:
            raise ValueError(f'Duplicate native build event: {package_id}')
        seen.add(package_id)
        try:
            receipt['components'].append(native_component(m, packages[package_id]))
        except (ValueError, OSError, KeyError) as error:
            receipt['blockers'].append(str(error))
    captured = {b['name']: b for b in capture['binaries']}
    if set(captured) != set(artifacts):
        raise ValueError('Captured binary set differs from Cargo artifact set')
    strip_receipt = None
    if args.strip_tool:
        strip_path = Path(run(['which', args.strip_tool]).strip()).resolve(strict=True)
        strip_receipt = evidence(strip_path)
    for name, path in artifacts.items():
        expected = (args.bin_dir / (name + ('.exe' if 'windows' in args.target else ''))).resolve()
        if path != expected or Path(captured[name]['path']).resolve() != path:
            raise ValueError(f'Artifact path mismatch: {name}')
        verify_evidence(captured[name])
        before = evidence(path)
        transform = {'kind': 'identity'}
        if strip_receipt:
            subprocess.run([strip_receipt['path'], str(path)], check=True)
            transform = {'kind': 'strip', 'tool': strip_receipt,
                         'command': [strip_receipt['path'], str(path)]}
        after = evidence(path)
        receipt['binaries'].append({'name': name, 'before': before, 'after': after,
                                    'transformation': transform, 'loader': loader(path, args.target)})
    try:
        receipt['runtime'] = runtime(capture['toolchain'], args.target)
    except (ValueError, OSError) as error:
        receipt['blockers'].append(str(error))
    if 'apple-darwin' not in args.target:
        receipt['blockers'].append('System linker C runtime/native origin licensing not yet proven for this platform')
    if 'apple-darwin' in args.target:
        linked = {p for b in receipt['binaries'] for p in b['loader']['libraries']}
        for c in receipt['components']:
            if packages[c['package_id']]['name'] == 'libz-sys' and not c['archives']:
                if '/usr/lib/libz.1.dylib' not in linked:
                    receipt['blockers'].append('Unqualified z link is not proven by system libz loader evidence')
    receipt['complete'] = not receipt['blockers']
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    if not receipt['complete']:
        raise ValueError('Native provenance incomplete: ' + '; '.join(receipt['blockers']))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    build = commands.add_parser('build')
    build.add_argument('--target', required=True)
    build.add_argument('--output', type=Path, required=True)
    build.add_argument('--native-host', action='store_true')
    build.add_argument('--metadata', type=Path, required=True)
    build.add_argument('--dependency-ids', type=Path, required=True)
    generate = commands.add_parser('produce')
    for option in ('metadata', 'dependency-ids', 'build-messages', 'bin-dir', 'output'):
        generate.add_argument('--' + option, type=Path, required=True)
    generate.add_argument('--target', required=True)
    generate.add_argument('--strip-tool')
    args = parser.parse_args()
    try:
        (capture_build if args.command == 'build' else produce)(args)
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'Native release evidence error: {error}\n')


if __name__ == '__main__':
    main()
