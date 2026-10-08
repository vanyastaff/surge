"""Independent invalid-input checks for the notice collector."""
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import release_notices as notices


class NoticeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def crate(self, entries):
        path = self.root / "demo-1.0.0.crate"
        with tarfile.open(path, "w:gz") as archive:
            for name, data in entries:
                member = tarfile.TarInfo("demo-1.0.0/" + name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_exact_text_and_nested_native_notice_are_preserved(self):
        source = b"Copyright Example\nMIT permission text  \n"
        path, checksum = self.crate([("LICENSE", source), ("vendor/lib/COPYING", b"Native terms\n"), ("src/lib.rs", b"code")])
        result = notices.archive_notices(path, checksum, "demo", "1.0.0")
        self.assertEqual([n["path"] for n in result], ["LICENSE", "vendor/lib/COPYING"])
        self.assertEqual(result[0]["text"].encode(), source)

    def test_literal_native_copyright_preserves_full_adjacent_context(self):
        source = b'/* permission and disclaimer */\nconst char *s = "Copyright 1995-2024 Authors";\n'
        result = notices.native_source_comments(source, "vendor/deflate.c")
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["text"].encode(), source)
        self.assertEqual((result[0]["byte_start"], result[0]["byte_end"]), (0, len(source)))
        for path in ["vendor/version.rc", "cmake/CMakeLists.txt", "cmake/rules.cmake"]:
            with self.subTest(path=path):
                result = notices.native_source_comments(b'LegalCopyright "Authors"\n', path)
                self.assertEqual(result[0]["text"], 'LegalCopyright "Authors"\n')

    def test_vendor_mapping_requires_exact_inventory_recipe_and_linkage(self):
        row = {"id": "demo", "crate_sha256": "crate", "notice_sources": [{"path": "LICENSE", "sha256": "text", "text": "terms"}]}
        inventory = [{k: v for k, v in n.items() if k != "text"} for n in row["notice_sources"]]
        review = {"package_name": "demo", "package_version": "1", "target": "target", "origin_kind": "crate-vendor",
                  "crate_sha256": "crate", "notice_inventory_sha256": notices.digest(notices.canonical(inventory).encode()),
                  "build_recipe_sha256": "recipe", "libraries": ["native"]}
        (self.root / "native-reviewed.json").write_text(json.dumps({"reviews": [review]}))
        data = {"target": "target", "metadata_document": {"packages": [{"id": "demo", "name": "demo", "version": "1", "manifest_path": "/src/demo/Cargo.toml"}]},
                "components": [{"package_id": "demo", "linked_libs": ["static=native"], "origin": {"kind": "crate-vendor", "version": "1", "source": "registry+https://github.com/rust-lang/crates.io-index", "root": "/src/demo", "build_recipe": {"path": "/src/demo/build.rs", "sha256": "recipe"}}}]}
        with patch.object(notices, "SUPPLEMENTS", self.root), patch.object(notices, "runtime_mapping_gap", return_value=None):
            self.assertEqual(notices.native_mapping_gaps(data, [row]), [])
            for key in ["sha256", "path"]:
                with self.subTest(key=key):
                    original = data["components"][0]["origin"]["build_recipe"][key]
                    data["components"][0]["origin"]["build_recipe"][key] = "tampered"
                    self.assertTrue(notices.native_mapping_gaps(data, [row]))
                    data["components"][0]["origin"]["build_recipe"][key] = original
            row["notice_sources"][0]["sha256"] = "tampered"
            self.assertTrue(notices.native_mapping_gaps(data, [row]))

    def test_exact_reviewed_correspondence_scope_and_text_tamper(self):
        reviews = json.loads((notices.SUPPLEMENTS / "reviewed-correspondence.json").read_text())["reviews"]
        for review in reviews:
            package = {"name": review["name"], "version": review["version"]}
            accepted, extras = notices.reviewed_correspondence(package, review["crate_sha256"], "aarch64-apple-darwin")
            self.assertIsNotNone(accepted)
            self.assertEqual(bool(extras), not review["name"].startswith("lineark"))
            self.assertIsNone(notices.reviewed_correspondence(package, "wrong", "aarch64-apple-darwin")[0])
            windows, _ = notices.reviewed_correspondence(package, review["crate_sha256"], "x86_64-pc-windows-msvc")
            self.assertEqual(windows is not None, review["name"].startswith("lineark"))
        for source in notices.SUPPLEMENTS.iterdir():
            if source.is_file():
                (self.root / source.name).write_bytes(source.read_bytes())
        (self.root / "canonical-MIT.txt").write_text("truncated permission")
        review = next(r for r in reviews if r["name"] == "objc2")
        with self.assertRaisesRegex(ValueError, "supplement changed"):
            notices.reviewed_correspondence({"name": "objc2", "version": review["version"]}, review["crate_sha256"], "aarch64-apple-darwin", self.root)

    def test_published_source_member_disposition_rejects_changed_source(self):
        source = b"original compiled source"
        archive, _ = self.crate([("src/lib.rs", source)])
        review = {"published_members_sha256": {"src/lib.rs": notices.digest(source)}}
        notices.verify_reviewed_members(archive, review)
        archive, _ = self.crate([("src/lib.rs", b"changed source")])
        with self.assertRaisesRegex(ValueError, "source members changed"):
            notices.verify_reviewed_members(archive, review)

    def test_stale_native_workspace_cannot_be_rebound_to_new_notice_source(self):
        old_files = [["src/lib.rs", "old"]]
        old = {"files": old_files, "sha256": notices.digest(notices.canonical(old_files).encode())}
        native = {"source_inputs": {"workspace_identity": old}}
        notices.verify_workspace_binding(native, old)
        new_files = [["src/lib.rs", "new"]]
        new = {"files": new_files, "sha256": notices.digest(notices.canonical(new_files).encode())}
        with self.assertRaisesRegex(ValueError, "workspace identity"):
            notices.verify_workspace_binding(native, new)
        with self.assertRaisesRegex(ValueError, "workspace identity"):
            notices.verify_workspace_binding({}, old)
        forged = {"files": new_files, "sha256": old["sha256"]}
        with self.assertRaisesRegex(ValueError, "workspace identity"):
            notices.verify_workspace_binding({"source_inputs": {"workspace_identity": forged}}, forged)

    def test_mutated_crate_fails_before_source_selection(self):
        path, checksum = self.crate([("LICENSE", b"terms")])
        path.write_bytes(path.read_bytes() + b"tampered")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            notices.archive_notices(path, checksum, "demo", "1.0.0")

    def test_empty_duplicate_and_traversal_members_fail(self):
        for entries in [[("LICENSE", b"")], [("LICENSE", b"a"), ("LICENSE", b"b")], [("../LICENSE", b"a")]]:
            with self.subTest(entries=entries):
                path, checksum = self.crate(entries)
                with self.assertRaises(ValueError):
                    notices.archive_notices(path, checksum, "demo", "1.0.0")

    def test_dirty_supplement_is_authentic_evidence_but_incomplete(self):
        data = b"Copyright publisher\npermission\n"
        (self.root / "LICENSE").write_bytes(data)
        mapping = {"name": "demo", "version": "1", "crate_sha256": "locked", "git_sha": "abc",
                   "published_vcs_dirty": True, "texts": [{"file": "LICENSE", "upstream_path": "LICENSE",
                   "url": "https://raw.githubusercontent.com/p/r/abc/LICENSE", "sha256": notices.digest(data)}]}
        (self.root / "index.json").write_text(json.dumps([mapping]))
        texts, gaps = notices.supplement_notices({"name": "demo", "version": "1"}, "locked", self.root)
        self.assertEqual(texts[0]["text"].encode(), data)
        self.assertEqual(len(gaps), 1)
        (self.root / "LICENSE").write_text("changed")
        with self.assertRaisesRegex(ValueError, "Supplement checksum"):
            notices.supplement_notices({"name": "demo", "version": "1"}, "locked", self.root)

    def test_native_comments_keep_unicode_byte_offsets_and_adjacent_clauses(self):
        source = "/* Copyright © Owner */\n/* permission paragraph */\n/* THE SOFTWARE IS PROVIDED AS IS */\nint x;".encode()
        result = notices.native_source_comments(source, "native/file.c")
        self.assertEqual(len(result), 1)
        for block in result:
            self.assertEqual(block["text"].encode(), source[block["byte_start"]:block["byte_end"]])
        self.assertIn("THE SOFTWARE", result[-1]["text"])

    def test_title_only_runtime_source_cannot_prove_complete_terms(self):
        source = "Copyright notices for The Rust Standard Library: synthetic fixture"
        gap = notices.runtime_mapping_gap({"toolchain": {"version": "release: 1.98.1\ncommit-hash: 48a229ceaefd4985c50990b14116b6d856af0985"},
                    "copyright": {"text": source, "sha256": notices.digest(source.encode())}})
        self.assertIsNotNone(gap)

    def test_source_file_named_copying_is_not_license_evidence(self):
        self.assertFalse(notices.is_notice("src/copying.rs"))
        self.assertTrue(notices.is_notice("LICENSE/APACHE"))

    def test_duplicate_json_keys_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "Duplicate JSON"):
            notices.strict_json('{"complete":false,"complete":true}')

    def test_forged_empty_complete_receipt_is_rejected(self):
        coverage = {"schema": 1, "target": "aarch64-apple-darwin", "complete": True, "gaps": [],
                    "cargo_lock_sha256": "lock", "source": {"sha256": "source"}, "packages": []}
        text = notices.canonical(coverage).encode()
        data = {"coverage": coverage, "notice_sha256": notices.digest(text), "coverage_sha256": notices.digest(text)}
        with self.assertRaisesRegex(ValueError, "package coverage"):
            notices.verify_embedded_receipt("aarch64-apple-darwin", data, text,
                                            expected_lock_sha256="lock", expected_source_sha256="source")

    def test_unknown_supplement_cannot_be_invented(self):
        (self.root / "index.json").write_text("[]")
        with self.assertRaisesRegex(ValueError, "Unknown supplemental"):
            notices.supplement_notices({"name": "missing", "version": "1"}, "checksum", self.root)

    def test_incomplete_diagnostic_receipt_cannot_be_packaged(self):
        (self.root / notices.TEXT).write_text("diagnostic")
        (self.root / notices.RECEIPT).write_text(json.dumps({"coverage": {
            "schema": 1, "target": "aarch64-apple-darwin", "complete": False, "gaps": ["native missing"]}}))
        with self.assertRaisesRegex(ValueError, "Incomplete"):
            notices.verify_receipt("aarch64-apple-darwin", self.root)

    def test_complete_mini_graph_uses_real_portable_validator(self):
        # Synthetic graph uses a real complete permission text, not a boolean bypass.
        crate_source = next(Path.home().joinpath(".cargo/registry/src").glob("*/itoa-1.0.18"))
        crate_checksum = "8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682"
        package = {"name": "itoa", "version": "1.0.18", "manifest_path": str(crate_source / "Cargo.toml")}
        archive = notices.registry_archive(package)
        members = notices.archive_members(archive, crate_checksum, "itoa", "1.0.18")
        with tarfile.open(archive) as stream:
            permission = stream.extractfile("itoa-1.0.18/LICENSE-MIT").read().decode()
        review = notices.strict_json((notices.SUPPLEMENTS / "runtime-reviewed.json").read_text())["reviews"][0]
        runtime_text = (notices.SUPPLEMENTS / review["file"]).read_text()
        runtime_version = f"release: {review['release']}\ncommit-hash: {review['commit']}"
        source = notices.source_identity()
        native = {"source_inputs": {"workspace_identity": source,
                    "authenticated_registry": {"registry-demo": {"checksum": crate_checksum,
                        "source": "registry+https://github.com/rust-lang/crates.io-index", "archive": {"sha256": crate_checksum}, "published_files": members}},
                    "packages": {"registry-demo": [{"path": str(crate_source / path), "sha256": sha} for path, sha in members.items()]}}, "schema": 1, "target": "aarch64-apple-darwin", "cargo_lock": {"sha256": "lock"},
                  "complete": True, "blockers": [], "components": [],
                  "binaries": [{"name": name, "after": {"path": "/fixture/" + name, "sha256": notices.digest(name.encode())},
                                "loader": {"libraries": ["system"]}} for name in ("surge", "surge-daemon")],
                  "build_messages_document": [{"reason": "compiler-artifact", "target": {"name": name},
                       "profile": {"test": False}, "executable": "/fixture/" + name} for name in ("surge", "surge-daemon")]
                       + [{"reason": "build-finished", "success": True}],
                  "dependency_ids_document": ["registry-demo"],
                  "metadata_document": {"packages": [{"id": "registry-demo", "source": "registry+https://github.com/rust-lang/crates.io-index", "manifest_path": str(crate_source / "Cargo.toml")}]},
                  "runtime": {"toolchain": {"version": runtime_version}, "copyright": {"text": runtime_text, "sha256": notices.digest(runtime_text.encode())}}}
        coverage = {"schema": 1, "target": "aarch64-apple-darwin", "complete": True, "gaps": [],
                    "cargo_lock_sha256": "lock", "source": source,
                    "packages": [{"id": "registry-demo", "name": "itoa", "version": "1.0.18",
                        "crate_sha256": crate_checksum, "published_files": members, "gaps": [], "notice_sources": [{"path": "LICENSE", "text": permission,
                                                                                 "sha256": notices.digest(permission.encode())}]}],
                    "native": {"receipt": native}}
        def receipt():
            text = (notices.canonical(coverage) + "\n===== itoa 1.0.18 / LICENSE =====\n" + permission + "\n").encode()
            return {"coverage": coverage, "notice_sha256": notices.digest(text),
                    "coverage_sha256": notices.digest(notices.canonical(coverage).encode())}, text
        def verify(data, text):
            return notices.verify_embedded_receipt("aarch64-apple-darwin", data, text,
                  expected_lock_sha256="lock", expected_source_sha256=source["sha256"], expected_graph_ids={"registry-demo"},
                  expected_package_checksums={("itoa", "1.0.18"): crate_checksum}, expected_published_files={"registry-demo": members})
        data, text = receipt()
        self.assertIs(verify(data, text), data)
        authentic_inputs = json.loads(json.dumps(native["source_inputs"]))
        published_path = next(iter(members))
        for mutation in ("absent", "empty", "missing_member", "changed_member", "unknown_package", "missing_captured", "unknown_captured"):
            with self.subTest(mutation=mutation):
                native["source_inputs"] = json.loads(json.dumps(authentic_inputs))
                auth = native["source_inputs"]["authenticated_registry"]
                if mutation == "absent":
                    del native["source_inputs"]["authenticated_registry"]
                elif mutation == "empty":
                    auth.clear()
                elif mutation == "missing_member":
                    del auth["registry-demo"]["published_files"][published_path]
                elif mutation == "changed_member":
                    auth["registry-demo"]["published_files"][published_path] = "mutated"
                elif mutation == "unknown_package":
                    auth["unselected"] = auth["registry-demo"]
                elif mutation == "missing_captured":
                    native["source_inputs"]["packages"]["registry-demo"].pop()
                else:
                    native["source_inputs"]["packages"]["registry-demo"].append({"path": str(crate_source / "target/hidden.rs"), "sha256": "unknown"})
                data, text = receipt()  # Every outer hash regenerated independently.
                with self.assertRaisesRegex(ValueError, "registry"):
                    verify(data, text)
        native["source_inputs"] = authentic_inputs
        stale_files = [["stale.rs", "old"]]
        native["source_inputs"]["workspace_identity"] = {"files": stale_files, "sha256": notices.digest(notices.canonical(stale_files).encode())}
        data, text = receipt()  # Rehash every outer field; source mismatch must still fail.
        with self.assertRaisesRegex(ValueError, "workspace identity"):
            verify(data, text)
        native["source_inputs"]["workspace_identity"] = source
        native["build_messages_document"].insert(0, {"reason": "build-script-executed", "package_id": "registry-demo", "linked_libs": ["static=undispositioned"]})
        native["components"] = [{"package_id": "registry-demo", "linked_libs": ["static=undispositioned"],
                                 "archives": [{"library": "undispositioned"}],
                                 "origin": {"kind": "crate-vendor", "root": "/fixture"}}]
        data, text = receipt()
        with self.assertRaisesRegex(ValueError, "registry-demo / static=undispositioned"):
            verify(data, text)

    def test_native_unknown_static_library_fails(self):
        events = [{"reason": "build-script-executed", "package_id": "pkg", "linked_libs": ["static=unknown"]},
                  {"reason": "build-finished", "success": True}]
        stream = self.root / "messages.jsonl"
        stream.write_text("\n".join(json.dumps(e) for e in events))
        native = self.root / "native.json"
        native.write_text(json.dumps({"schema": 1, "target": "aarch64-apple-darwin", "cargo_lock": {"sha256": "lock"},
              "build_messages": {"path": str(stream), "sha256": notices.digest(stream.read_bytes())}, "components": []}))
        with self.assertRaisesRegex(ValueError, "static-library coverage"):
            notices.native_coverage(native, "aarch64-apple-darwin", "lock", {"pkg"})


if __name__ == "__main__":
    unittest.main()
