"""Package native builds and collect the exact supported release set (Python 3.11+)."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import tarfile
import tomllib
import zipfile

from release_notices import (TEXT, digest, source_identity, strict_json,
                             verify_embedded_receipt, verify_receipt)

TARGETS = (
    "x86_64-pc-windows-msvc", "x86_64-apple-darwin",
    "aarch64-apple-darwin", "x86_64-unknown-linux-gnu",
)
DOCUMENTS = ("README.md", "LICENSE-MIT", "LICENSE-APACHE", TEXT)


def archive_name(target):
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"surge-{target}{suffix}"


def receipt_name(target):
    return f"surge-{target}.notices.json"


def file_digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def publish_files(staged, output):
    """Publish checked files without replacing existing candidates; roll back our links on error."""
    destinations = [output / path.name for path in staged]
    if any(path.exists() or path.is_symlink() for path in destinations):
        raise ValueError(f"Release destination already exists: {output}")
    output.mkdir(parents=True, exist_ok=True)
    published = []
    try:
        for source, destination in zip(staged, destinations):
            os.link(source, destination)
            published.append(destination)
    except OSError:
        for path in published:
            path.unlink()
        raise


def package(args):
    windows = "windows" in args.target
    binaries = [name + (".exe" if windows else "") for name in ("surge", "surge-daemon")]
    files = [(args.bin_dir / name, name) for name in binaries]
    files.extend((Path(name), name) for name in DOCUMENTS if name != TEXT)
    files.append((args.notices_dir / TEXT, TEXT))
    for source, _ in files:
        if source.is_symlink() or not source.is_file() or source.stat().st_size == 0:
            raise ValueError(f"Missing or empty package input: {source}")
    proof = verify_receipt(args.target, args.notices_dir)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".surge-package-", dir=args.output.parent) as temporary:
        stage = Path(temporary)
        destination = stage / archive_name(args.target)
        if windows:
            with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
                for source, name in files:
                    archive.write(source, name)
        else:
            with tarfile.open(destination, "w:gz") as archive:
                for source, name in files:
                    info = archive.gettarinfo(str(source), arcname=name)
                    info.mode = 0o755 if name in binaries else 0o644
                    with source.open("rb") as stream:
                        archive.addfile(info, stream)
        contents = validate_archive(destination)
        verify_embedded_receipt(args.target, proof, contents["notices"], contents["binaries"])
        pairing = {"schema": 1, "target": args.target,
                   "archive": {"name": destination.name, "sha256": file_digest(destination)},
                   "notice_sha256": digest(contents["notices"]),
                   "binary_sha256": contents["binaries"], "collector": proof}
        receipt = stage / receipt_name(args.target)
        receipt.write_text(json.dumps(pairing, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        validate_pair(args.target, destination, receipt)
        # Publish the archive last; an interrupted pair is rejected by collect's exact-set gate.
        publish_files([receipt, destination], args.output)


def validate_pair(target, archive, receipt, source_hash=None):
    data = strict_json(receipt.read_text(encoding="utf-8"))
    contents = validate_archive(archive)
    if (data["schema"] != 1 or data["target"] != target
            or data["archive"]["name"] != archive_name(target)
            or data["archive"]["sha256"] != file_digest(archive)
            or data["notice_sha256"] != digest(contents["notices"])
            or data["binary_sha256"] != contents["binaries"]):
        raise ValueError("Release archive/provenance binding mismatch")
    verify_embedded_receipt(target, data["collector"], contents["notices"], contents["binaries"],
                            expected_source_sha256=source_hash)


def collect(args):
    expected = {name for target in TARGETS for name in (archive_name(target), receipt_name(target))}
    found = {}
    for path in args.input.rglob("*"):
        if path.is_dir() and not path.is_symlink():
            continue
        if path.name not in expected or path.name in found:
            raise ValueError(f"Unexpected or duplicate release asset: {path}")
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Empty or invalid release asset: {path}")
        found[path.name] = path
    if set(found) != expected:
        raise ValueError(f"Missing release assets: {sorted(expected - set(found))}")
    source_hash = source_identity()["sha256"]
    for target in TARGETS:
        validate_pair(target, found[archive_name(target)], found[receipt_name(target)], source_hash)
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError(f"Output must be empty: {args.output}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".surge-collect-", dir=args.output.parent) as temporary:
        stage = Path(temporary)
        lines = []
        for name, source in sorted(found.items()):
            destination = stage / name
            shutil.copyfile(source, destination)
            lines.append(f"{file_digest(destination)}  {name}\n")
        # Revalidate staged bytes so mutations during copying cannot escape the gate.
        for target in TARGETS:
            validate_pair(target, stage / archive_name(target), stage / receipt_name(target), source_hash)
        (stage / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")
        publish_files(sorted(stage.iterdir()), args.output)


def drain(stream):
    """Read every byte to check decompression/CRC without retaining binaries in memory."""
    checksum = hashlib.sha256()
    while chunk := stream.read(1024 * 1024):
        checksum.update(chunk)
    return checksum.hexdigest()


def validate_archive(path):
    windows = path.suffix == ".zip"
    binaries = {name + (".exe" if windows else "") for name in ("surge", "surge-daemon")}
    expected = binaries | set(DOCUMENTS)
    result = {"binaries": {}, "notices": b""}
    try:
        if windows:
            with zipfile.ZipFile(path) as archive:
                members = archive.infolist()
                if len(members) != len(expected) or {m.filename for m in members} != expected:
                    raise ValueError("expected exactly the required unique package members")
                for member in members:
                    kind = stat.S_IFMT(member.external_attr >> 16)
                    if (member.is_dir() or member.external_attr & 0x10
                            or kind not in (0, stat.S_IFREG) or member.file_size == 0):
                        raise ValueError(f"not a nonempty regular file: {member.filename}")
                    with archive.open(member) as stream:
                        checksum = drain(stream)
                    if member.filename in binaries:
                        result["binaries"][member.filename.removesuffix(".exe")] = checksum
                    elif member.filename == TEXT:
                        result["notices"] = archive.read(member)
        else:
            # tarfile can stop at the tar end marker before checking the gzip trailer.
            with gzip.open(path, "rb") as stream:
                drain(stream)
            with tarfile.open(path, "r:gz") as archive:
                members = archive.getmembers()
                if len(members) != len(expected) or {m.name for m in members} != expected:
                    raise ValueError("expected exactly the required unique package members")
                for member in members:
                    if not member.isfile() or member.size == 0:
                        raise ValueError(f"not a nonempty regular file: {member.name}")
                    if member.name in binaries and member.mode & 0o111 != 0o111:
                        raise ValueError(f"missing executable bits: {member.name}")
                    with archive.extractfile(member) as stream:
                        checksum = drain(stream)
                    if member.name in binaries:
                        result["binaries"][member.name] = checksum
                    elif member.name == TEXT:
                        with archive.extractfile(member) as stream:
                            result["notices"] = stream.read()
    except (ValueError, OSError, EOFError, tarfile.TarError, zipfile.BadZipFile,
            RuntimeError, NotImplementedError) as error:
        raise ValueError(f"Invalid release archive {path}: {error}") from error
    return result


def metadata(args):
    with Path("Cargo.toml").open("rb") as stream:
        version = tomllib.load(stream)["workspace"]["package"]["version"]
    # Cargo validates SemVer when building; this also excludes path/output injection.
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
        raise ValueError(f"Invalid workspace version: {version}")
    tagged = args.ref.startswith("refs/tags/")
    if tagged and args.ref != f"refs/tags/v{version}":
        raise ValueError(f"Release tag must match workspace version v{version}")
    print(f"publish={str(tagged).lower()}")
    print(f"prerelease={str('-' in version.split('+')[0]).lower()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    pack = sub.add_parser("package")
    pack.add_argument("--target", choices=TARGETS, required=True)
    pack.add_argument("--bin-dir", type=Path, required=True)
    pack.add_argument("--output", type=Path, required=True)
    pack.add_argument("--notices-dir", type=Path, required=True)
    gather = sub.add_parser("collect")
    gather.add_argument("--input", type=Path, required=True)
    gather.add_argument("--output", type=Path, required=True)
    meta = sub.add_parser("metadata")
    meta.add_argument("--ref", required=True)
    args = parser.parse_args()
    try:
        {"package": package, "collect": collect, "metadata": metadata}[args.command](args)
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
