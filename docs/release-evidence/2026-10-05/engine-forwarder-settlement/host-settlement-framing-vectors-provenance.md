# Independent host-settlement framing vectors

Author: independent architecture reviewer `/root/caller_plan_arch`, at root request.
Contract: accepted durable-settlement plan SHA-256 `9b509500aa927a3368c0ebfe2d931c71fca035ea6c08f34ce5a365d0e789ac54`, exact JournalPrefixDigestV1 and terminal-row framing paragraphs.
Vector file SHA-256: `ed530d62dda9b695bf098617f8a62e4206a33011ad3186e1b1a87536536290c7`.

Six fixed vectors cover Unix and Windows at stored schema versions1/2 with one identical raw RunCompleted JSON blob, plus empty prefix0 for each platform. Canonical RunId Display is `run-01ARZ3NDEKTSV4RRFFQ69G5FAV` (30 UTF-8 bytes), not serde's bare ULID. Domain including NUL is40 bytes. Timestamp -2 encodes as `feffffffffffffff`; sequence1, version1/2, all length prefixes and identity integers are explicitly literal little-endian hex blocks. Windows FileId.Identifier arrays stay in exact SDK byte order, never GUID/endian reinterpretation. Identity order is home, run directory, original journal lock, journal DB. Fixed identities are synthetic, not observed Windows execution evidence.

Independent Python assembly concatenated the literal hex blocks retained verbatim in each JSON vector and applied only `bytes.fromhex` and `hashlib.sha256` to form expected digests. It did not call/import/run any production framing encoder, Rust code, Cargo, native SDK or migration implementation. Integer decoding was used only to describe literal identity inputs. JSON serialization formatted the vector document, never reserialized its raw event payload. Every one-row row is121 bytes; Unix headers147/prefixes268 bytes, Windows headers179/prefixes300 bytes. The terminal hash is SHA-256 of the exact121 row bytes only. The same row has the same terminal digest on both platforms; the prefix includes platform/identities and differs.

Source inspection found MIN_SUPPORTED_VERSION=1, MAX_SUPPORTED_VERSION=21, and a complete IdentityV1..IdentityV21 default registry. IdentityV1 and IdentityV2 deserialize VersionedEventPayload and return wrapper.payload; neither requires embedded wrapper.schema_version to equal the SQL column. Thus identical raw wrapper-v1 bytes decode through both stored-v1 and stored-v2 paths according to current source. This is source-derived, not a claimed executed Rust migration test. No new embedded-version equality policy is invented. RunCompleted uses snake_case JSON type `run_completed`, NodeKey string `end`, and SQL discriminant `RunCompleted`. One terminal row is valid row decoding/framing, but is NOT asserted to form a valid folded complete run or grant host confirmation.

Negative descriptions cover signed/overflow/schema range/missing migration, kind mismatch/UTF-8, malformed payload, sequence domain/gap/duplicate, extra suffix including zero prefix, same-snapshot count, RunId spelling and platform/identity ordering. The missing-migration case is explicitly conditional: current default registry has no missing supported version;22 is too-new, not a missing migration. Integer negatives are rejection inputs, not hashable normalized replacements.

Read-only source snapshot hashes:
- `crates/surge-core/src/id.rs`: `052b2e0a1356cf8763065da5fbf73e15a03604c576f6c2fead8e52e47ba0f323`
- `crates/surge-core/src/run_event.rs`: `5ea7a2449afdb2cf4340a06212a479295ec7e9f54ac90923486511de2abc1468`
- `crates/surge-core/src/keys.rs`: `bd892b87ac8524350ab7fee08306d79eb7bc589251b8bfbd75b0a39b3de1e062`
- `crates/surge-core/src/migrations/mod.rs`: `64d08657a49b23980af594de383f181dfa98ab6f0a5ffa517ef67a5c167108aa`
- `crates/surge-persistence/src/state_home/windows/namespace.rs`: `b0abd47d73ed9ff657c93af187ec42d4414491ae3c208fd0cf09479dbf9d92cf`

Only this provenance and vector JSON were written. No Rust/schema changes, builds, native tests or acceptance claims. An independent consumer/reviewer must check the literals before using them as production-encoder assertions; this author does not self-certify implementation.
