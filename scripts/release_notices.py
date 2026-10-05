"""Collect source notices bound to locked production dependencies (Python 3.11+).

Inventory mode records unresolved evidence; it never creates a packageable receipt.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import tomllib

TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin",
           "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc")
TEXT = "THIRD_PARTY_NOTICES.txt"
RECEIPT = "THIRD_PARTY_NOTICES.provenance.json"
SUPPLEMENTS = Path(__file__).parent / "notice-sources"


def strict_json(text):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError(f"Duplicate JSON key: {key}")
            result[key] = value
        return result
    return json.loads(text, object_pairs_hook=pairs)


def source_identity():
    tracked = subprocess.run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                             check=True, capture_output=True).stdout.split(b"\0")
    entries = []
    for raw in sorted(set(tracked) - {b""}):
        path = Path(raw.decode())
        if path.is_symlink():
            value = digest(str(path.readlink()).encode())
        elif path.is_file():
            value = digest(path.read_bytes())
        else:
            value = "absent"
        entries.append([str(path), value])
    return {"sha256": digest(canonical(entries).encode()), "files": entries}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def checked_text(data, label):
    text = data.decode("utf-8")
    if not text.strip():
        raise ValueError(f"Empty notice: {label}")
    return text


def is_notice(path):
    parts = PurePosixPath(path).parts
    if not parts or PurePosixPath(path).suffix.lower() in (".rs", ".c", ".h", ".cpp", ".py", ".toml"):
        return False
    return (bool(re.match(r"(?i)^(license|licence|copying|copyright|notice|authors)([._-]|$)", parts[-1]))
            or any(p.upper() in ("LICENSE", "LICENSES", "LICENCE", "NOTICES") for p in parts[:-1]))


NATIVE_PACKAGES = {"libgit2-sys", "libssh2-sys", "ring", "psm", "libsqlite3-sys", "mac-notification-sys", "libz-sys"}


def native_source_comments(data, path):
    """Preserve complete comment runs; offsets refer to authenticated original bytes."""
    if PurePosixPath(path).suffix not in (".c", ".h", ".S", ".s", ".asm", ".m", ".rs", ".pl", ".cc", ".cpp", ".inc", ".rc", ".cmake") and PurePosixPath(path).name != "CMakeLists.txt":
        return []
    marker = re.compile(rb"/\*|(?m:^[ \t]*(?://|#|@|;))")
    line_marker = re.compile(rb"[ \t]*(?://|#|@|;)")
    ranges = []
    position = 0
    while match := marker.search(data, position):
        start = match.start()
        if match.group() == b"/*":
            closing = data.find(b"*/", match.end())
            if closing < 0:
                break
            end = closing + 2
        else:
            end = data.find(b"\n", match.end())
            end = len(data) if end < 0 else end + 1
            while end < len(data) and line_marker.match(data, end):
                following = data.find(b"\n", end)
                end = len(data) if following < 0 else following + 1
        ranges.append((start, end))
        position = end
    # Merge adjacent comments before selecting: permission and disclaimer blocks
    # may be separate comments immediately following the copyright block.
    groups = []
    for start, end in ranges:
        if groups and not data[groups[-1][1]:start].strip():
            groups[-1] = (groups[-1][0], end)
        else:
            groups.append((start, end))
    ranges = [(start, end) for start, end in groups
              if re.search(rb"(?i)copyright|public domain|permission is hereby|redistribution|licen[cs]e|SPDX", data[start:end])]
    # Literal declarations outside comments may carry redistributable notices.
    # Preserve the entire authenticated file to retain every adjacent clause.
    unmatched = [m for m in re.finditer(rb"(?i)copyright|LegalCopyright", data)
                 if not any(start <= m.start() < end for start, end in ranges)]
    if unmatched:
        ranges = [(0, len(data))]
    result = []
    member_hash = digest(data)
    for start, end in ranges:
        block = data[start:end]
        # Keep each complete licensing comment group with adjacent disclaimers;
        # retain exact original bytes, not individual keyword-matching lines.
        result.append({"path": f"{path}:bytes={start}-{end}",
                       "source_member": path, "source_member_sha256": member_hash,
                       "byte_start": start, "byte_end": end,
                       "sha256": digest(block), "text": checked_text(block, path),
                       "origin": "published-crate-native-source-comment"})
    return result


def archive_notices(path, checksum, name, version):
    """Read authenticated archive bytes, not a potentially modified source checkout."""
    if digest(path.read_bytes()) != checksum:
        raise ValueError(f"Crate archive checksum mismatch: {name}@{version}")
    notices = []
    prefix = f"{name}-{version}/"
    with tarfile.open(path, "r:gz") as archive:
        seen = set()
        for member in archive.getmembers():
            if not member.name.startswith(prefix):
                raise ValueError(f"Unexpected crate archive root: {member.name}")
            relative = member.name[len(prefix):]
            parts = PurePosixPath(relative).parts
            if ".." in parts or relative.startswith("/") or relative in seen:
                raise ValueError(f"Unsafe or duplicate crate member: {relative}")
            seen.add(relative)
            if not member.isfile():
                continue
            named_notice = is_notice(relative)
            native_source = name in NATIVE_PACKAGES
            if not named_notice and not native_source:
                continue
            with archive.extractfile(member) as stream:
                data = stream.read()
            if named_notice:
                notices.append({"path": relative, "sha256": digest(data),
                                "text": checked_text(data, relative), "origin": "published-crate"})
            elif native_source:
                notices.extend(native_source_comments(data, relative))
    return sorted(notices, key=lambda n: n["path"])


def supplement_notices(package, checksum, root=SUPPLEMENTS):
    index = strict_json((root / "index.json").read_text())
    matches = [p for p in index if p["name"] == package["name"] and p["version"] == package["version"]]
    if len(matches) != 1 or matches[0]["crate_sha256"] != checksum:
        raise ValueError(f"Unknown supplemental source mapping: {package['name']}@{package['version']}")
    mapping = matches[0]
    notices = []
    for item in mapping["texts"]:
        path = (root / item["file"]).resolve()
        if not path.is_relative_to(root.resolve()):
            raise ValueError("Supplement path escapes source directory")
        data = path.read_bytes()
        if digest(data) != item["sha256"]:
            raise ValueError(f"Supplement checksum mismatch: {item['file']}")
        if f"/{mapping['git_sha']}/" not in item["url"]:
            raise ValueError("Supplement URL is not bound to immutable revision")
        notices.append({"path": item["upstream_path"], "sha256": digest(data),
                        "text": checked_text(data, path), "origin": item["url"]})
    gaps = []
    if mapping.get("published_vcs_dirty"):
        gaps.append("published source marked dirty; upstream notice correspondence unverified")
    if package["name"] in ("objc2", "objc2-encode", "objc2-foundation", "objc2-core-foundation", "block2"):
        gaps.append("upstream licensing explanation is not complete permission/attribution evidence; SDK-derived obligations unverified")
    return notices, gaps


def run_json(args):
    return strict_json(subprocess.run(args, check=True, capture_output=True, text=True).stdout)


def production_graph(target):
    metadata = run_json(["cargo", "metadata", "--locked", "--format-version", "1"])
    command = ["cargo", "tree", "--locked", "--offline", "-p", "surge-cli", "-p", "surge-daemon",
               "--target", target, "--edges", "normal,build", "--prefix", "none", "--format", "{p}|{f}"]
    lines = subprocess.run(command, check=True, capture_output=True, text=True).stdout.splitlines()
    by_key = {}
    for package in metadata["packages"]:
        by_key.setdefault((package["name"], package["version"]), []).append(package)
    selected = {}
    for line in lines:
        if not line.strip():
            continue  # Cargo separates the two requested root trees with a blank line.
        match = re.fullmatch(r"([^ ]+) v([^ |]+)(?: \([^|]*\))?\|([^*]*?)(?: \(\*\))?", line)
        if not match or len(by_key.get(match.group(1, 2), [])) != 1:
            raise ValueError(f"Ambiguous production dependency identity: {line}")
        package = by_key[match.group(1, 2)][0]
        selected[package["id"]] = (package, match[3].strip())
    return [selected[k] for k in sorted(selected)], command, metadata


def verify_file(record):
    path = Path(record["path"])
    if not path.is_file() or path.is_symlink() or digest(path.read_bytes()) != record["sha256"]:
        raise ValueError(f"Missing or changed provenance input: {path}")


def runtime_mapping_gap(runtime):
    if runtime is None:
        return "Rust standard-library/sysroot copyright inventory missing"
    document = runtime["copyright"]
    if digest(document["text"].encode()) != document["sha256"]:
        raise ValueError("Rust runtime notice source hash mismatch")
    version = runtime.get("toolchain", {}).get("version", "")
    reviews = strict_json((SUPPLEMENTS / "runtime-reviewed.json").read_text())["reviews"]
    for review in reviews:
        source = (SUPPLEMENTS / review["file"]).read_bytes()
        if digest(source) != review["sha256"]:
            raise ValueError("Trusted Rust runtime notice source changed")
        if (f"release: {review['release']}" in version and f"commit-hash: {review['commit']}" in version
                and document["sha256"] == review["sha256"]):
            return None
    return "Rust runtime notice/toolchain commit lacks exact reviewed source mapping"


def native_mapping_gaps(data, package_rows=()):
    """Report exact selected origins not independently reviewed, never blanket completion."""
    gaps = []
    reviews = strict_json((SUPPLEMENTS / "native-reviewed.json").read_text())["reviews"]
    metadata = {p["id"]: p for p in data.get("metadata_document", {}).get("packages", [])}
    lock = tomllib.loads(Path("Cargo.lock").read_text())
    checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock["package"]}
    rows = {p["id"]: p for p in package_rows}
    for component in data.get("components", []):
        static = [lib for lib in component.get("linked_libs", []) if lib.startswith("static=")]
        if not static:
            continue
        origin = component.get("origin", {})
        package = metadata.get(component["package_id"], {})
        candidates = [r for r in reviews if r["package_name"] == package.get("name")
                      and r["package_version"] == package.get("version") and r["target"] == data.get("target")]
        approved = False
        if len(candidates) == 1:
            review = candidates[0]
            if review["origin_kind"] == "crate-vendor":
                row = rows.get(component["package_id"], {})
                inventory = [{k: v for k, v in n.items() if k != "text"} for n in row.get("notice_sources", [])]
                approved = (row.get("crate_sha256") == review["crate_sha256"]
                            and digest(canonical(inventory).encode()) == review["notice_inventory_sha256"]
                            and origin.get("kind") == "crate-vendor"
                            and origin.get("version") == review["package_version"]
                            and origin.get("source") == "registry+https://github.com/rust-lang/crates.io-index"
                            and origin.get("build_recipe", {}).get("sha256") == review["build_recipe_sha256"]
                            and {lib.split("=", 1)[1] for lib in static} == set(review["libraries"])
                            and origin.get("root") == str(PurePosixPath(package.get("manifest_path", "")).parent)
                            and origin.get("build_recipe", {}).get("path") == str(PurePosixPath(origin["root"]) / "build.rs"))
            else:
                approved = (checksums.get((review["package_name"], review["package_version"])) == review["crate_sha256"]
                        and origin.get("kind") == review["origin_kind"] and origin.get("version") == review["origin_version"]
                        and {lib.split("=", 1)[1] for lib in static} == set(review["libraries"])
                        and set(review["proof_sha256"]) <= {f["sha256"] for f in origin.get("proof_files", [])}
                        and set(review["license_sha256"]) == {f["sha256"] for f in origin.get("licenses", [])}
                        and {a["library"]: a["sha256"] for a in component.get("archives", [])} == review["archive_sha256"])
            if approved and review["origin_kind"] == "system-package":
                for record in origin.get("proof_files", []) + origin.get("licenses", []) + component.get("archives", []):
                    if not PurePosixPath(record["path"]).is_relative_to(PurePosixPath(origin["root"])):
                        raise ValueError("Reviewed native source evidence escapes its root")
        if approved:
            continue
        for library in static:
            gaps.append(f"native {component['package_id']} / {library} / {origin.get('kind', 'unknown')} "
                        f"at {origin.get('root', 'unknown')}: source/header notice mapping not independently reviewed")
    runtime_gap = runtime_mapping_gap(data.get("runtime"))
    if runtime_gap:
        gaps.append(runtime_gap)
    gaps.extend(data.get("blockers", []))
    return gaps


def native_coverage(path, target, lock_hash, package_ids, package_rows=()):
    """Native receipts require independent reviewed mappings; hashes alone are not provenance."""
    if path is None:
        return None, ["native production linkage receipt missing"]
    data = strict_json(path.read_text())
    if data.get("schema") != 1 or data.get("target") != target or data.get("cargo_lock", {}).get("sha256") != lock_hash:
        raise ValueError("Native receipt target/schema/lock mismatch")
    verify_file(data["build_messages"])
    events = [strict_json(line) for line in Path(data["build_messages"]["path"]).read_text().splitlines()]
    if not events or events[-1].get("reason") != "build-finished" or events[-1].get("success") is not True:
        raise ValueError("Native receipt lacks successful final Cargo build event")
    builds = {e["package_id"]: e for e in events if e.get("reason") == "build-script-executed"
              and e["package_id"] in package_ids}
    static = {(key, lib.split("=", 1)[1]) for key, e in builds.items() for lib in e.get("linked_libs", []) if lib.startswith("static=")}
    covered = set()
    for component in data.get("components", []):
        key = component["package_id"]
        if key not in builds or component["linked_libs"] != builds[key].get("linked_libs", []):
            raise ValueError("Native component does not match actual selected build event")
        verify_file(component["build_output"])
        if component.get("origin", {}).get("kind") == "crate-vendor":
            verify_file(component["origin"]["build_recipe"])
        for record in component.get("archives", []):
            verify_file(record)
            covered.add((key, record["library"]))
        for record in (component.get("licenses", []) + component.get("origin", {}).get("proof_files", [])
                       + component.get("origin", {}).get("licenses", [])):
            verify_file(record)
    if covered != static:
        raise ValueError(f"Native static-library coverage mismatch: {sorted(static - covered)}")
    data["build_messages_document"] = events
    names = set()
    for binary in data.get("binaries", []):
        verify_file(binary["after"])
        if not binary.get("loader", {}).get("libraries"):
            raise ValueError("Binary loader evidence missing")
        names.add(binary["name"])
    if names != {"surge", "surge-daemon"}:
        raise ValueError("Native receipt must bind both production binaries")
    for key in ("metadata", "dependency_ids"):
        verify_file(data[key])
    data["metadata_document"] = strict_json(Path(data["metadata"]["path"]).read_text())
    data["dependency_ids_document"] = strict_json(Path(data["dependency_ids"]["path"]).read_text())
    runtime = data.get("runtime")
    if runtime is not None:
        verify_file(runtime["copyright"])
        if digest(runtime["copyright"]["text"].encode()) != runtime["copyright"]["sha256"]:
            raise ValueError("Rust runtime notice text differs from source")
    # Do not accept a user-set complete boolean as independent native source review.
    gaps = native_mapping_gaps(data, package_rows)
    return {"path": str(path.resolve()), "sha256": digest(path.read_bytes()), "receipt": data}, gaps


def collect(target, output, native=None, inventory=False):
    lock_bytes = Path("Cargo.lock").read_bytes()
    lock = tomllib.loads(lock_bytes.decode())
    checksums = {(p["name"], p["version"], p.get("source")): p.get("checksum") for p in lock["package"]}
    packages, command, _ = production_graph(target)
    inventory_rows, texts, gaps = [], [], []
    for package, features in packages:
        if package["source"] is None:
            if package["name"] not in {"surge-cli", "surge-daemon", "surge-core", "surge-process", "surge-acp",
                    "surge-git", "surge-persistence", "surge-orchestrator", "surge-notify", "surge-telegram",
                    "surge-intake", "surge-mcp", "surge-ui"}:
                raise ValueError(f"Unknown local dependency notice source: {package['id']}")
            continue
        if package["source"] != "registry+https://github.com/rust-lang/crates.io-index":
            raise ValueError(f"Unknown registry/git source: {package['id']}")
        checksum = checksums.get((package["name"], package["version"], package["source"]))
        if not checksum:
            raise ValueError(f"Missing lock checksum: {package['id']}")
        source = Path(package["manifest_path"]).parent
        cache = source.parents[2] / "cache" / source.parent.name / f"{package['name']}-{package['version']}.crate"
        notices = archive_notices(cache, checksum, package["name"], package["version"])
        package_gaps = []
        if not notices:
            notices, package_gaps = supplement_notices(package, checksum)
        if not notices:
            raise ValueError(f"No complete notice sources: {package['id']}")
        row = {"id": package["id"], "name": package["name"], "version": package["version"],
               "license_expression": package["license"], "features": features, "crate_sha256": checksum,
               "notice_sources": notices, "gaps": package_gaps}
        inventory_rows.append(row)
        gaps.extend(f"{package['name']}@{package['version']}: {g}" for g in package_gaps)
        texts.extend(f"\n===== {package['name']} {package['version']} / {n['path']} =====\n{n['text']}\n" for n in notices)
    native_record, native_gaps = native_coverage(native, target, digest(lock_bytes), {p["id"] for p, _ in packages}, inventory_rows)
    gaps.extend(native_gaps)
    if native_record:
        for component in native_record["receipt"].get("components", []):
            for item in component.get("origin", {}).get("licenses", []):
                if "text" not in item or digest(item["text"].encode()) != item["sha256"]:
                    raise ValueError("Native external license text is missing or changed")
                texts.append("\n===== Native " + component["package_id"] + " / " + item["path"] + " =====\n" + item["text"])
    if native_record and native_record["receipt"].get("runtime"):
        texts.append("\n===== Rust standard library and runtime notices =====\n" + native_record["receipt"]["runtime"]["copyright"]["text"])
    else:
        gaps.append("Rust standard-library/sysroot copyright inventory missing")
    coverage = {"schema": 1, "target": target, "cargo_lock_sha256": digest(lock_bytes),
                "graph_command": command, "scope": "CLI/daemon default features, normal and build dependencies",
                "complete": not gaps, "gaps": gaps, "packages": inventory_rows, "native": native_record,
                "source": source_identity()}
    body = "Surge production dependency notice inventory\nCoverage receipt (canonical JSON):\n" + canonical(coverage) + "\n" + "".join(texts)
    receipt = {"coverage": coverage, "notice_sha256": digest(body.encode()), "coverage_sha256": digest(canonical(coverage).encode())}
    if gaps and not inventory:
        raise ValueError("Notice coverage incomplete:\n" + "\n".join(gaps))
    output.mkdir(parents=True, exist_ok=True)
    (output / TEXT).write_text(body, encoding="utf-8")
    (output / RECEIPT).write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return receipt


def verify_embedded_receipt(target, data, text, binary_hashes=None, *, expected_lock_sha256=None,
                            expected_source_sha256=None, expected_graph_ids=None, expected_package_checksums=None):
    """Portable validation against independently supplied frozen checkout identities."""
    coverage = data["coverage"]
    expected_lock_sha256 = expected_lock_sha256 or digest(Path("Cargo.lock").read_bytes())
    expected_source_sha256 = expected_source_sha256 or source_identity()["sha256"]
    if (coverage["schema"] != 1 or coverage["target"] != target or coverage["complete"] is not True
            or coverage["gaps"] or coverage["cargo_lock_sha256"] != expected_lock_sha256
            or coverage["source"]["sha256"] != expected_source_sha256
            or data["notice_sha256"] != digest(text)
            or data["coverage_sha256"] != digest(canonical(coverage).encode())
            or canonical(coverage).encode() not in text):
        raise ValueError("Incomplete, stale or tampered notice receipt")
    identities = [(p["name"], p["version"], p["crate_sha256"]) for p in coverage["packages"]]
    if not identities or len(set(identities)) != len(identities) or any(p["gaps"] for p in coverage["packages"]):
        raise ValueError("Duplicate or unresolved package coverage")
    for package in coverage["packages"]:
        if not package.get("notice_sources"):
            raise ValueError("Package full notice text missing")
        for notice in package["notice_sources"]:
            source = checked_text(notice["text"].encode(), notice["path"])
            block = f"\n===== {package['name']} {package['version']} / {notice['path']} =====\n{source}\n"
            if digest(source.encode()) != notice["sha256"] or block.encode() not in text:
                raise ValueError("Package notice source text/hash mismatch")
    # Producer hashes are evidence, not authority to retire independently unresolved mappings.
    native = coverage["native"]
    if native is None or native["receipt"].get("complete") is not True:
        raise ValueError("Incomplete native source coverage")
    if (native["receipt"].get("schema") != 1 or native["receipt"].get("target") != target
            or native["receipt"].get("cargo_lock", {}).get("sha256") != expected_lock_sha256):
        raise ValueError("Portable native target/schema/lock mismatch")
    blocked_names = {"lineark-sdk", "lineark-derive", "objc2", "objc2-encode", "objc2-foundation", "objc2-core-foundation", "block2"}
    if any(p["name"] in blocked_names for p in coverage["packages"]):
        raise ValueError("Independently unresolved source-license correspondence")
    expected_ids = set(native["receipt"]["dependency_ids_document"])
    if expected_graph_ids is None:
        graph, _, _ = production_graph(target)
        expected_graph_ids = {p["id"] for p, _ in graph}
    if expected_ids != set(expected_graph_ids):
        raise ValueError("Production graph differs from frozen expected graph")
    actual_ids = {p["id"] for p in coverage["packages"]}
    metadata = {p["id"]: p for p in native["receipt"]["metadata_document"]["packages"]}
    third_party_ids = {key for key in expected_ids if metadata[key]["source"] is not None}
    if actual_ids != third_party_ids:
        raise ValueError("Production graph package coverage mismatch")
    if expected_package_checksums is None:
        lock = tomllib.loads(Path("Cargo.lock").read_text())
        expected_package_checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock["package"]}
    for package in coverage["packages"]:
        if expected_package_checksums.get((package["name"], package["version"])) != package["crate_sha256"]:
            raise ValueError("Production package checksum differs from frozen lock")
    binaries = native["receipt"].get("binaries", [])
    recorded = {b["name"]: b["after"]["sha256"] for b in binaries}
    if len(binaries) != 2 or set(recorded) != {"surge", "surge-daemon"}:
        raise ValueError("Portable receipt must bind both production binaries")
    if binary_hashes is not None:
        if recorded != binary_hashes:
            raise ValueError("Native final binary hashes mismatch")
    events = native["receipt"].get("build_messages_document", [])
    finished = [event for event in events if event.get("reason") == "build-finished"]
    if len(finished) != 1 or events[-1] != finished[0] or finished[0].get("success") is not True:
        raise ValueError("Portable native build proof missing unique successful stream")
    artifacts = {}
    for event in events:
        if event.get("reason") == "compiler-artifact" and event.get("executable"):
            name = event.get("target", {}).get("name")
            if name in {"surge", "surge-daemon"}:
                if name in artifacts or event.get("profile", {}).get("test"):
                    raise ValueError("Duplicate or test production binary artifact")
                artifacts[name] = event["executable"]
    if set(artifacts) != {"surge", "surge-daemon"}:
        raise ValueError("Portable Cargo proof missing production artifacts")
    for binary in binaries:
        if binary["after"]["path"] != artifacts[binary["name"]] or not binary.get("loader", {}).get("libraries"):
            raise ValueError("Portable binary artifact/loader mismatch")
    builds = {e["package_id"]: e for e in events if e.get("reason") == "build-script-executed"}
    observed = {(key, lib.split("=", 1)[1]) for key, event in builds.items()
                for lib in event.get("linked_libs", []) if lib.startswith("static=")}
    covered = set()
    for component in native["receipt"].get("components", []):
        key = component["package_id"]
        if key not in builds or component["linked_libs"] != builds[key].get("linked_libs", []):
            raise ValueError("Portable native component/build mismatch")
        covered.update((key, archive["library"]) for archive in component.get("archives", []))
    if observed != covered:
        raise ValueError("Portable native static coverage mismatch")
    mapping_gaps = native_mapping_gaps(native["receipt"], coverage["packages"])
    if mapping_gaps:
        raise ValueError("Unresolved native mappings: " + "; ".join(mapping_gaps))
    return data


def verify_receipt(target, notices_dir):
    notices_dir = Path(notices_dir)
    data = strict_json((notices_dir / RECEIPT).read_text())
    verify_embedded_receipt(target, data, (notices_dir / TEXT).read_bytes(),
                            expected_lock_sha256=digest(Path("Cargo.lock").read_bytes()),
                            expected_source_sha256=source_identity()["sha256"])
    native = data["coverage"]["native"]
    if digest(Path(native["path"]).read_bytes()) != native["sha256"]:
        raise ValueError("Missing or changed native receipt")
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--native-provenance", type=Path)
    parser.add_argument("--graph-only", action="store_true", help="prepare exact production IDs and metadata for native capture")
    parser.add_argument("--inventory-only", action="store_true", help="write explicitly incomplete diagnostic evidence, never packageable")
    args = parser.parse_args()
    if args.graph_only:
        packages, command, metadata = production_graph(args.target)
        args.output_dir.mkdir(parents=True, exist_ok=True)
        (args.output_dir / "production-metadata.json").write_text(json.dumps(metadata) + "\n")
        (args.output_dir / "production-dependency-ids.json").write_text(json.dumps([p["id"] for p, _ in packages]) + "\n")
        return
    collect(args.target, args.output_dir, args.native_provenance, args.inventory_only)


if __name__ == "__main__":
    main()
