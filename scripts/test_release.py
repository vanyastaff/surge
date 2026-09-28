"""Black-box release contract checks; run with unittest discover."""
import hashlib
import io
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

SCRIPT = Path(__file__).with_name("release.py")
TARGETS = (
    "x86_64-unknown-linux-gnu", "x86_64-apple-darwin",
    "aarch64-apple-darwin", "x86_64-pc-windows-msvc",
)


class ReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ("README.md", "LICENSE-MIT", "LICENSE-APACHE"):
            (self.root / name).write_text(name)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        self.bins = self.root / "bin"
        self.bins.mkdir()
        for name in ("surge", "surge-daemon", "surge.exe", "surge-daemon.exe"):
            (self.bins / name).write_bytes(b"binary fixture: " + name.encode())

    def run_script(self, *args, ok=True):
        result = subprocess.run([sys.executable, str(SCRIPT), *map(str, args)],
                                cwd=self.root, capture_output=True, text=True)
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def package(self, target):
        out = self.root / "artifacts" / target
        self.run_script("package", "--target", target, "--bin-dir", self.bins,
                        "--output", out)
        suffix = ".zip" if "windows" in target else ".tar.gz"
        return out / ("surge-" + target + suffix)

    def test_archives_and_independent_checksums(self):
        archives = [self.package(target) for target in TARGETS]
        for archive in archives:
            windows = archive.suffix == ".zip"
            binaries = {"surge.exe", "surge-daemon.exe"} if windows else {"surge", "surge-daemon"}
            expected = binaries | {"README.md", "LICENSE-MIT", "LICENSE-APACHE"}
            if windows:
                with zipfile.ZipFile(archive) as z:
                    self.assertEqual(set(z.namelist()), expected)
                    self.assertEqual(len(z.namelist()), len(expected))
                    content = {name: z.read(name) for name in expected}
            else:
                with tarfile.open(archive) as t:
                    self.assertEqual(set(t.getnames()), expected)
                    self.assertEqual(len(t.getmembers()), len(expected))
                    for name in binaries:
                        self.assertEqual(t.getmember(name).mode & 0o777, 0o755)
                    content = {name: t.extractfile(name).read() for name in expected}
            for name, data in content.items():
                source = self.bins / name if name in binaries else self.root / name
                self.assertEqual(data, source.read_bytes())
        out = self.root / "release-assets"
        self.run_script("collect", "--input", self.root / "artifacts", "--output", out)
        self.assertEqual({p.name for p in out.iterdir()}, {p.name for p in archives} | {"SHA256SUMS"})
        lines = (out / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(lines), 4)
        for line in lines:
            digest, name = line.split("  ")
            self.assertEqual(digest, hashlib.sha256((out / name).read_bytes()).hexdigest())

    def test_invalid_collections(self):
        for problem in ("missing", "duplicate", "unexpected", "empty"):
            with self.subTest(problem=problem):
                archives = [self.package(target) for target in TARGETS]
                extra = self.root / "artifacts" / "extra"
                extra.mkdir(exist_ok=True)
                if problem == "missing":
                    archives[0].unlink()
                elif problem == "duplicate":
                    (extra / archives[0].name).write_bytes(archives[0].read_bytes())
                elif problem == "unexpected":
                    (extra / "unexpected.zip").write_bytes(b"wrong")
                else:
                    archives[0].write_bytes(b"")
                result = self.run_script("collect", "--input", self.root / "artifacts",
                                         "--output", self.root / "release-assets", ok=False)
                reason = {"missing": "Missing release archives", "duplicate": "duplicate",
                          "unexpected": "Unexpected", "empty": "Empty"}[problem]
                self.assertIn(reason, result.stderr)
                self.assertFalse((self.root / "release-assets").exists())
                for path in extra.iterdir():
                    path.unlink()

    def test_missing_or_empty_daemon_rejected(self):
        daemon = self.bins / "surge-daemon"
        daemon.unlink()
        for _ in range(2):
            result = self.run_script("package", "--target", TARGETS[0], "--bin-dir", self.bins,
                                     "--output", self.root / "out", ok=False)
            self.assertIn("Missing or empty package input", result.stderr)
            daemon.touch()

    def test_malformed_archives_rejected_before_output(self):
        cases = ("garbage", "truncated", "corrupt-checksum", "wrong-member", "duplicate", "empty-member",
                 "not-executable", "symlink", "traversal")
        for target in (TARGETS[0], TARGETS[3]):
            for problem in cases:
                if "windows" in target and problem == "not-executable":
                    continue
                with self.subTest(target=target, problem=problem):
                    out = self.root / (target + "-" + problem)
                    archives = {t: self.package(t) for t in TARGETS}
                    bad = archives[target]
                    if problem == "garbage":
                        bad.write_bytes(b"not an archive")
                    elif problem == "truncated":
                        bad.write_bytes(bad.read_bytes()[:len(bad.read_bytes()) // 2])
                    elif problem == "corrupt-checksum":
                        data = bytearray(bad.read_bytes())
                        offset = data.index(b"PK\x01\x02") + 16 if bad.suffix == ".zip" else len(data) - 8
                        data[offset] ^= 1
                        bad.write_bytes(data)
                    else:
                        windows = bad.suffix == ".zip"
                        daemon = "surge-daemon.exe" if windows else "surge-daemon"
                        names = ["surge.exe" if windows else "surge", daemon,
                                 "README.md", "LICENSE-MIT", "LICENSE-APACHE"]
                        if problem == "wrong-member":
                            names[1] = "other"
                        elif problem == "duplicate":
                            names.append(daemon)
                        elif problem == "traversal":
                            names[1] = "../" + daemon
                        if windows:
                            with zipfile.ZipFile(bad, "w") as archive:
                                for name in names:
                                    info = zipfile.ZipInfo(name)
                                    info.create_system = 3
                                    info.external_attr = (0o120777 if problem == "symlink" and name == daemon
                                                          else 0o100644) << 16
                                    archive.writestr(info, b"" if problem == "empty-member" and name == daemon else b"fixture")
                        else:
                            with tarfile.open(bad, "w:gz") as archive:
                                for name in names:
                                    info = tarfile.TarInfo(name)
                                    info.mode = 0o644 if problem == "not-executable" else 0o755
                                    data = b"" if problem == "empty-member" and name == daemon else b"fixture"
                                    info.size = len(data)
                                    if problem == "symlink" and name == daemon:
                                        info.type = tarfile.SYMTYPE
                                        info.linkname = "surge"
                                        info.size = 0
                                    archive.addfile(info, io.BytesIO(data))
                    result = self.run_script("collect", "--input", self.root / "artifacts",
                                             "--output", out, ok=False)
                    self.assertIn("Invalid release archive", result.stderr)
                    self.assertFalse(out.exists())

    def test_tag_validation_and_prerelease(self):
        for version in ("0.1.0", "0.1.0-rc.1", "0.1.0+build.1", "0.1.0+build-with-hyphen"):
            (self.root / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{version}"\n')
            output = self.run_script("metadata", "--ref", f"refs/tags/v{version}").stdout
            self.assertIn("prerelease=" + ("true" if version == "0.1.0-rc.1" else "false"), output)
            self.assertIn("publish=true", output)
        self.run_script("metadata", "--ref", "refs/tags/v9.9.9", ok=False)
        self.run_script("metadata", "--ref", "refs/tags/v0.1.0/evil", ok=False)
        output = self.run_script("metadata", "--ref", "refs/heads/main").stdout
        self.assertIn("publish=false", output)


if __name__ == "__main__":
    unittest.main()
