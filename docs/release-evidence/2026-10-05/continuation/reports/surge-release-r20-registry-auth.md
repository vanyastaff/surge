# R20 authenticated registry source review

Verdict: ACCEPTABLE for scoped change. No shared script edits and no Cargo commands.

Read-only reviewed authenticate_registry_source + snapshot_sources: exact locked source/name/version/checksum, archive path safety, published source byte comparison, unknown-file and symlink rejection; registry source walk preserves nested target directories.

Independent oracle: /tmp/surge-release-r20-registry-auth.py run with /opt/homebrew/bin/python3.12. Nine checks PASS: exact valid archive retains src/target/real.rs; source tamper rejected; archive checksum tamper rejected; omitted published nested target file rejected; unknown extra source rejected; published file symlink rejected; published directory symlink rejected; archive symlink rejected; full snapshot_sources in temporary Git repository retains nested target file and authenticated published SHA. No patched validation or mirrored scanner oracle; expected nested path fixed literal and expected hash from original fixture bytes.

Native owner suite:14 tests PASS in0.470s after final symlink strengthening.

The initial snapshot assertion used /var fixture path rather than canonical /private path on macOS; oracle corrected to resolve fixed expected path. Production retained file correctly.

Remaining proof: root controlled native recapture and complete release gates at final committed state. This review does not substitute for production recapture.
