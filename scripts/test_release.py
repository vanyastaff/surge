"""Black-box release contract checks; run with unittest discover."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

import release_notices as notices

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
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["surge-cli", "surge-daemon"]\n'
            '[workspace.package]\nversion = "0.1.0"\n')
        (self.root / ".gitignore").write_text("/target/\n")
        for name in ("surge-cli", "surge-daemon"):
            crate = self.root / name
            (crate / "src").mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
                '[dependencies]\nitoa = "=1.0.18"\n')
            (crate / "src/main.rs").write_text("fn main() {}\n")
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "itoa"\nversion = "1.0.18"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682"\n'
            '[[package]]\nname = "surge-cli"\nversion = "0.1.0"\ndependencies = ["itoa"]\n'
            '[[package]]\nname = "surge-daemon"\nversion = "0.1.0"\ndependencies = ["itoa"]\n')
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        self.bins = self.root / "target/bin"
        self.bins.mkdir(parents=True)
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

    def notice_fixture(self, target):
        """Minimal synthetic no-native-library proof; never actual Surge release evidence."""
        folder = self.root / "target/notices" / target
        folder.mkdir(parents=True, exist_ok=True)
        original = Path.cwd()
        try:
            os.chdir(self.root)
            packages, command, metadata = notices.production_graph(target)
            source = notices.source_identity()
        finally:
            os.chdir(original)
        lock_hash = hashlib.sha256((self.root / "Cargo.lock").read_bytes()).hexdigest()
        review = notices.strict_json((notices.SUPPLEMENTS / "runtime-reviewed.json").read_text())["reviews"][0]
        runtime_text = (notices.SUPPLEMENTS / review["file"]).read_text()
        runtime_version = f"release: {review['release']}\ncommit-hash: {review['commit']}"
        native = {"schema": 1, "target": target, "complete": True, "blockers": [],
                  "components": [], "cargo_lock": {"sha256": lock_hash},
                  "dependency_ids_document": [p["id"] for p, _ in packages],
                  "metadata_document": metadata,
                  "runtime": {"toolchain": {"version": runtime_version},
                              "copyright": {"text": runtime_text,
                                            "sha256": notices.digest(runtime_text.encode())}},
                  "binaries": [{"name": name, "before": {"sha256": hashlib.sha256(
                      (self.bins / (name + (".exe" if "windows" in target else ""))).read_bytes()).hexdigest()},
                      "after": {"path": str(self.bins / (name + (".exe" if "windows" in target else ""))),
                          "sha256": hashlib.sha256((self.bins / (name + (
                          ".exe" if "windows" in target else ""))).read_bytes()).hexdigest()},
                      "loader": {"libraries": ["system fixture"]},
                      "transformation": {"kind": "identity"}} for name in ("surge", "surge-daemon")]}
        native["build_messages_document"] = [
            {"reason": "compiler-artifact", "target": {"name": binary["name"]},
             "profile": {"test": False}, "executable": binary["after"]["path"]}
            for binary in native["binaries"]] + [{"reason": "build-finished", "success": True}]
        permission = Path(next(p["manifest_path"] for p, _ in packages if p["name"] == "itoa")).parent.joinpath("LICENSE-MIT").read_text()
        native_path = folder / "native.json"
        native_path.write_text(json.dumps(native))
        checksum = "8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682"
        rows = [{"id": p["id"], "name": p["name"], "version": p["version"],
                 "license_expression": p["license"], "features": features,
                 "crate_sha256": checksum,
                 "notice_sources": [{"path": "LICENSE-MIT", "text": permission,
                                      "sha256": hashlib.sha256(permission.encode()).hexdigest(),
                                      "origin": "synthetic-test"}], "gaps": []}
                for p, features in packages if p["source"] is not None]
        coverage = {"schema": 1, "target": target, "cargo_lock_sha256": lock_hash,
                    "graph_command": command, "scope": "synthetic test fixture",
                    "complete": True, "gaps": [], "packages": rows, "source": source,
                    "native": {"path": str(native_path), "sha256": hashlib.sha256(native_path.read_bytes()).hexdigest(),
                               "receipt": native}}
        blocks = "".join(f"\n===== {row['name']} {row['version']} / LICENSE-MIT =====\n{permission}\n" for row in rows)
        text = ("Synthetic packaging test; not a release readiness proof.\n" + notices.canonical(coverage) + blocks).encode()
        proof = {"coverage": coverage, "coverage_sha256": hashlib.sha256(notices.canonical(coverage).encode()).hexdigest(),
                 "notice_sha256": hashlib.sha256(text).hexdigest()}
        (folder / notices.TEXT).write_bytes(text)
        (folder / notices.RECEIPT).write_text(json.dumps(proof))
        return folder

    def package(self, target):
        out = self.root / "target/artifacts" / target
        suffix = ".zip" if "windows" in target else ".tar.gz"
        archive = out / ("surge-" + target + suffix)
        # Only fake fixtures owned by this test are regenerated; production refuses overwrite.
        for path in (archive, out / f"surge-{target}.notices.json"):
            path.unlink(missing_ok=True)
        self.run_script("package", "--target", target, "--bin-dir", self.bins,
                        "--notices-dir", self.notice_fixture(target), "--output", out)
        return archive

    def test_archives_and_independent_checksums(self):
        archives = [self.package(target) for target in TARGETS]
        for archive in archives:
            windows = archive.suffix == ".zip"
            binaries = {"surge.exe", "surge-daemon.exe"} if windows else {"surge", "surge-daemon"}
            expected = binaries | {"README.md", "LICENSE-MIT", "LICENSE-APACHE", notices.TEXT}
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
                source = (self.bins / name if name in binaries else
                          self.root / "target/notices" / archive.parent.name / name if name == notices.TEXT else self.root / name)
                self.assertEqual(data, source.read_bytes())
        out = self.root / "target/release-assets"
        self.run_script("collect", "--input", self.root / "target/artifacts", "--output", out)
        self.assertEqual({p.name for p in out.iterdir()}, {p.name for p in archives} | {f"surge-{target}.notices.json" for target in TARGETS} | {"SHA256SUMS"})
        lines = (out / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(lines), 8)
        for line in lines:
            digest, name = line.split("  ")
            self.assertEqual(digest, hashlib.sha256((out / name).read_bytes()).hexdigest())

    def test_invalid_collections(self):
        for problem in ("missing", "duplicate", "unexpected", "empty"):
            with self.subTest(problem=problem):
                archives = [self.package(target) for target in TARGETS]
                extra = self.root / "target/artifacts" / "extra"
                extra.mkdir(exist_ok=True)
                if problem == "missing":
                    archives[0].unlink()
                elif problem == "duplicate":
                    (extra / archives[0].name).write_bytes(archives[0].read_bytes())
                elif problem == "unexpected":
                    (extra / "unexpected.zip").write_bytes(b"wrong")
                else:
                    archives[0].write_bytes(b"")
                result = self.run_script("collect", "--input", self.root / "target/artifacts",
                                         "--output", self.root / "target/release-assets", ok=False)
                reason = {"missing": "Missing release assets", "duplicate": "duplicate",
                          "unexpected": "Unexpected", "empty": "Empty"}[problem]
                self.assertIn(reason, result.stderr)
                self.assertFalse((self.root / "target/release-assets").exists())
                for path in extra.iterdir():
                    path.unlink()

    def test_missing_or_empty_daemon_rejected(self):
        daemon = self.bins / "surge-daemon"
        folder = self.notice_fixture(TARGETS[0])
        daemon.unlink()
        for _ in range(2):
            result = self.run_script("package", "--target", TARGETS[0], "--bin-dir", self.bins,
                                     "--notices-dir", folder, "--output", self.root / "target/out", ok=False)
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
                    out = self.root / "target" / (target + "-" + problem)
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
                                 "README.md", "LICENSE-MIT", "LICENSE-APACHE", notices.TEXT]
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
                    result = self.run_script("collect", "--input", self.root / "target/artifacts",
                                             "--output", out, ok=False)
                    self.assertIn("Invalid release archive", result.stderr)
                    self.assertFalse(out.exists())

    def test_notice_receipt_failures_do_not_create_candidate(self):
        cases = ("missing-text", "empty-text", "missing-receipt", "tamper-text", "wrong-target",
                 "incomplete", "unresolved", "duplicate-key", "changed-native")
        for case in cases:
            with self.subTest(case=case):
                folder = self.notice_fixture(TARGETS[0])
                text = folder / notices.TEXT
                receipt = folder / notices.RECEIPT
                data = json.loads(receipt.read_text())
                if case == "missing-text":
                    text.unlink()
                elif case == "empty-text":
                    text.write_bytes(b"")
                elif case == "missing-receipt":
                    receipt.unlink()
                elif case == "tamper-text":
                    text.write_bytes(text.read_bytes() + b"changed")
                elif case == "duplicate-key":
                    receipt.write_text('{"coverage":{},"coverage":{}}')
                elif case == "changed-native":
                    (folder / "native.json").write_text("changed")
                else:
                    if case == "wrong-target":
                        data["coverage"]["target"] = TARGETS[3]
                    elif case == "incomplete":
                        data["coverage"]["complete"] = False
                    else:
                        data["coverage"]["gaps"] = ["unverified fixture origin"]
                    coverage = notices.canonical(data["coverage"]).encode()
                    text.write_bytes(coverage)
                    data["notice_sha256"] = hashlib.sha256(coverage).hexdigest()
                    data["coverage_sha256"] = hashlib.sha256(coverage).hexdigest()
                    receipt.write_text(json.dumps(data))
                output = self.root / "target" / ("notice-failure-" + case)
                self.run_script("package", "--target", TARGETS[0], "--bin-dir", self.bins,
                                "--notices-dir", folder, "--output", output, ok=False)
                self.assertFalse(output.exists())

    def test_existing_candidate_is_not_overwritten(self):
        archive = self.package(TARGETS[0])
        sidecar = archive.parent / f"surge-{TARGETS[0]}.notices.json"
        before = (archive.read_bytes(), sidecar.read_bytes())
        self.run_script("package", "--target", TARGETS[0], "--bin-dir", self.bins,
                        "--notices-dir", self.notice_fixture(TARGETS[0]),
                        "--output", archive.parent, ok=False)
        self.assertEqual((archive.read_bytes(), sidecar.read_bytes()), before)

    def test_collection_requires_valid_paired_provenance(self):
        for case in ("missing", "duplicate", "malformed", "wrong-target", "archive-hash", "binary-hash", "incomplete"):
            with self.subTest(case=case):
                self.package(TARGETS[0])
                for target in TARGETS[1:]:
                    self.package(target)
                receipt = self.root / "target/artifacts" / TARGETS[0] / f"surge-{TARGETS[0]}.notices.json"
                data = json.loads(receipt.read_text())
                if case == "missing":
                    receipt.unlink()
                elif case == "duplicate":
                    duplicate = self.root / "target/artifacts/duplicate"
                    duplicate.mkdir(exist_ok=True)
                    extra = duplicate / receipt.name
                    extra.write_bytes(receipt.read_bytes())
                    self.addCleanup(extra.unlink, missing_ok=True)
                elif case == "malformed":
                    receipt.write_text('{"schema":1,"schema":1}')
                else:
                    if case == "wrong-target":
                        data["target"] = TARGETS[3]
                    elif case == "archive-hash":
                        data["archive"]["sha256"] = "0" * 64
                    elif case == "binary-hash":
                        data["binary_sha256"]["surge"] = "0" * 64
                    else:
                        data["collector"]["coverage"]["complete"] = False
                    receipt.write_text(json.dumps(data))
                output = self.root / "target" / ("pair-failure-" + case)
                self.run_script("collect", "--input", self.root / "target/artifacts", "--output", output, ok=False)
                self.assertFalse(output.exists())
                for extra in (self.root / "target/artifacts/duplicate").glob("*"):
                    extra.unlink()

    def test_historical_five_member_archive_is_rejected(self):
        archive = self.package(TARGETS[0])
        with tarfile.open(archive, "w:gz") as output:
            for name in ("surge", "surge-daemon", "README.md", "LICENSE-MIT", "LICENSE-APACHE"):
                member = tarfile.TarInfo(name)
                member.mode = 0o755
                member.size = 7
                output.addfile(member, io.BytesIO(b"fixture"))
        for target in TARGETS[1:]:
            self.package(target)
        output = self.root / "target/five-member-rejected"
        self.run_script("collect", "--input", self.root / "target/artifacts", "--output", output, ok=False)
        self.assertFalse(output.exists())

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
