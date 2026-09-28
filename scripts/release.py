"""Package native builds and collect the exact supported release set (Python 3.11+)."""
import argparse
import gzip
import hashlib
from pathlib import Path
import re
import shutil
import stat
import tarfile
import tomllib
import zipfile

TARGETS = (
    "x86_64-pc-windows-msvc", "x86_64-apple-darwin",
    "aarch64-apple-darwin", "x86_64-unknown-linux-gnu",
)
DOCUMENTS = ("README.md", "LICENSE-MIT", "LICENSE-APACHE")


def archive_name(target):
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"surge-{target}{suffix}"


def package(args):
    windows = "windows" in args.target
    binaries = [name + (".exe" if windows else "") for name in ("surge", "surge-daemon")]
    files = [(args.bin_dir / name, name) for name in binaries]
    files.extend((Path(name), name) for name in DOCUMENTS)
    for source, _ in files:
        if not source.is_file() or source.stat().st_size == 0:
            raise ValueError(f"Missing or empty package input: {source}")
    args.output.mkdir(parents=True, exist_ok=True)
    destination = args.output / archive_name(args.target)
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


def collect(args):
    expected = {archive_name(target) for target in TARGETS}
    found = {}
    for path in args.input.rglob("*"):
        if path.is_dir():
            continue
        if path.name not in expected or path.name in found:
            raise ValueError(f"Unexpected or duplicate release archive: {path}")
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Empty or invalid release archive: {path}")
        found[path.name] = path
    if set(found) != expected:
        raise ValueError(f"Missing release archives: {sorted(expected - set(found))}")
    for path in found.values():
        validate_archive(path)
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError(f"Output must be empty: {args.output}")
    args.output.mkdir(parents=True, exist_ok=True)
    lines = []
    for name, source in sorted(found.items()):
        destination = args.output / name
        shutil.copyfile(source, destination)
        with destination.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    (args.output / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")


def drain(stream):
    """Read every byte to check decompression/CRC without retaining binaries in memory."""
    while stream.read(1024 * 1024):
        pass


def validate_archive(path):
    windows = path.suffix == ".zip"
    binaries = {name + (".exe" if windows else "") for name in ("surge", "surge-daemon")}
    expected = binaries | set(DOCUMENTS)
    try:
        if windows:
            with zipfile.ZipFile(path) as archive:
                members = archive.infolist()
                if len(members) != 5 or {m.filename for m in members} != expected:
                    raise ValueError("expected exactly the five unique package members")
                for member in members:
                    kind = stat.S_IFMT(member.external_attr >> 16)
                    if (member.is_dir() or member.external_attr & 0x10
                            or kind not in (0, stat.S_IFREG) or member.file_size == 0):
                        raise ValueError(f"not a nonempty regular file: {member.filename}")
                    with archive.open(member) as stream:
                        drain(stream)
        else:
            # tarfile can stop at the tar end marker before checking the gzip trailer.
            with gzip.open(path, "rb") as stream:
                drain(stream)
            with tarfile.open(path, "r:gz") as archive:
                members = archive.getmembers()
                if len(members) != 5 or {m.name for m in members} != expected:
                    raise ValueError("expected exactly the five unique package members")
                for member in members:
                    if not member.isfile() or member.size == 0:
                        raise ValueError(f"not a nonempty regular file: {member.name}")
                    if member.name in binaries and member.mode & 0o111 != 0o111:
                        raise ValueError(f"missing executable bits: {member.name}")
                    with archive.extractfile(member) as stream:
                        drain(stream)
    except (ValueError, OSError, EOFError, tarfile.TarError, zipfile.BadZipFile,
            RuntimeError, NotImplementedError) as error:
        raise ValueError(f"Invalid release archive {path}: {error}") from error


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
    gather = sub.add_parser("collect")
    gather.add_argument("--input", type=Path, required=True)
    gather.add_argument("--output", type=Path, required=True)
    meta = sub.add_parser("metadata")
    meta.add_argument("--ref", required=True)
    args = parser.parse_args()
    try:
        {"package": package, "collect": collect, "metadata": metadata}[args.command](args)
    except (ValueError, OSError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
