# G2a: pure guardian record shape and canonical digest

Status: owning-role maintainer verdict ACCEPTABLE as a bounded implementation
candidate. Independent pre-code API/serde and adversarial review remain required.
No Rust was edited and no Cargo command was run while preparing this plan.

## Scope and acceptance boundary

Implement only strict immutable DTOs, local constructor invariants, canonical byte
encoding and SHA-256. Add `pub mod guardian_ledger;` in
crates/surge-core/src/execution_recovery.rs and the new module and its private
wire/encoding/tests submodules as useful. No dependency changes are needed.
Existing G1 guardian.rs public types and existing wire bytes stay unchanged.

This slice does not add EventPayload, bump envelope version 21, migrate historical
journals, fold RunMemory, accept transcripts, modify admission, create a guardian,
query a process/Job, authenticate IPC or grant authority. G2b owns those next
historical/projection contracts. A valid receipt shape or matching hash is never
proof that its claimed observation occurred. ADR-0023 remains proposed.

Acceptance: independently specified wire literals produce exact typed fields,
exact canonical preimages and exact digests; each malformed boundary is rejected
by both direct construction (where expressible) and decoding. Optional wire values
are explicitly present nulls, never missing fields. Every existing G1/core test
continues to pass. Native Windows execution is not needed to prove this pure
slice; it remains mandatory for later guardian ownership closure.

## Verified current reuse map

- execution_recovery/guardian.rs: GuardianInstallationId has nonzero [u8;32]
  and as_bytes(); GuardianAuthority exposes installation_id(), executable_hash(),
  protocol_version() and accepts only protocol 1. Raw authority rejects hash aliases.
- WindowsGuardianProcessIdentity is positive PID u32 and creation_filetime u64,
  immutable, serde validated. It observes no process.
- WindowsGuardianIdentity has authority(), occurrence(), guardian(), child(),
  endpoint(); its constructor validates nonnil occurrence and distinct guardian/
  child PIDs, derives the 106-byte local label. It necessarily includes a child,
  so it cannot represent GuardianBound without fabricating one.
- WindowsGuardianContainer has version(), identity(), coverage(); its private
  constructor-fixed discriminator is windows_guardian/version 1/GroupOnly.
  Encode the fixed discriminator byte explicitly; no new kind accessor is needed.
- ContentHash::{as_bytes,compute,from_bytes} supplies SHA-256. Its general parser
  accepts bare hex/uppercase aliases: new raw wire fields must parse strings and
  require parsed.to_string() == original, as G1 does. All-zero digests remain valid.
- RunId, StageInvocationId, ExecutionWriterId expose nil() and as_ulid(). Their
  Display adds prefixes and FromStr accepts aliases. New raw context IDs must be
  strings: parse, reject nil, compare as_ulid().to_string() exactly to original.
  Canonical wire IDs are uppercase bare ULIDs; endpoint ULID text is lowercase.
- No pre-existing guardian ledger, canonical receipt encoder or public nullable
  presence wrapper exists. Do not repurpose Unix WriterContainer or legacy booleans.

## Public immutable surface

Use meaningful typed getters and validating new constructors; fields stay private,
with no Default, public mutable field, DerefMut or unvalidated deserialization.
`WindowsGuardianRecord::new(context, operation, predecessor, lease, body)` validates
all local relationships and fixes version=1. `hash() -> ContentHash` is infallible
for an already validated record. `canonical_bytes() -> Vec<u8>` exposes the public
preimage for independent audit, with no filesystem or authority effects.

Supporting types:

- GuardianRecordContext: run RunId, invocation StageInvocationId, occurrence
  ExecutionWriterId, authority GuardianAuthority; every ID nonnil.
- GuardianOperationId: nonzero 16-byte value, canonical 32 lowercase hex on wire;
  constructor accepts existing bytes only. Generation/randomness belongs to G3.
- Distinct positive u64 wrappers: GuardianLease, GuardianRegistryGeneration,
  GuardianBarrierGeneration, GuardianQueryGeneration, GuardianRevocationGeneration,
  GuardianHostObservationGeneration. Do not silently interchange the domains.
- GuardianJournalSequence: integer 1..=i64::MAX, matching the actual SQLite journal
  domain; it is not the unrestricted u64 lease domain.
- WindowsGuardianBinding: authority, occurrence, guardian process, derived endpoint,
  without child. A typed constructor derives endpoint from installation+occurrence;
  raw decode must match exact derived endpoint. A from_container helper can reuse
  G1 getters. Do not create a fake G1 child to reuse its constructor.
- GuardianRecordBody public tagged enum wraps concrete validated payload types;
  payload fields private and their own constructors validate local invariants.
  Do not expose public struct variants that bypass per-kind constructors.
- ResumeObservation: Started(previous_suspend_count == 1), Failed(win32_error:
  nonzero u32), Uncertain(no additional payload).
- GuardianTakeoverAuthorization: PreviousHostTerminated(host, lease,
  observation_generation) or PreviousHostRelinquished(host, lease, operation, hash).
  These are declared evidence data, not authenticated capabilities.
- GuardianSettlementProof: BoundJobEmpty(binding, barrier_generation,
  query_generation) or ChildJobEmpty(container, barrier_generation, query_generation).

Wire fields below are exact. Body tags are snake_case and numeric tags in the
encoding table are frozen independently of enum declaration order.

| Tag | Wire body kind | Fields, in canonical encoding order |
|---|---|---|
| 1 | launch_intent | host, registry_generation |
| 2 | cancelled_before_spawn | launch_operation, host, registry_generation, revocation_generation |
| 3 | guardian_bound | binding, host, registry_generation |
| 4 | lease_taken_over | binding, previous_lease, previous_host, host, previous_registry_generation, registry_generation, authorization |
| 5 | child_established | container |
| 6 | resume_authorized | container, barrier_generation |
| 7 | resume_observed | container, resume_operation, barrier_generation, outcome |
| 8 | settlement_requested | binding, barrier_generation |
| 9 | settlement_observed | settlement_operation, proof |
| 10 | settlement_consumed | settlement_operation, settlement_hash, settlement_sequence, barrier_generation, query_generation |

Common outer wire fields, in order for hashing (JSON key order is irrelevant):
version, context, operation, predecessor, lease, body. Context fields are run,
invocation, occurrence, authority. `predecessor` and `lease` are mandatory fields
whose values can be null only as specified below.

## Local checks versus deferred transcript checks

Record constructor rejects:

- version other than 1 on decode; nil/invalid IDs, noncanonical raw spellings,
  invalid positive/ranged counters, unknown fields/kinds/outcomes;
- launch_intent unless predecessor and lease are null;
- cancelled_before_spawn unless predecessor is present and lease null;
- guardian_bound unless predecessor present and lease exactly 1;
- any other body unless predecessor and positive lease are present;
- any embedded binding/container/proof whose authority or occurrence differs from
  context. Compare exact G1 values, not only endpoint/PID;
- guardian_bound when host PID equals guardian PID;
- takeover unless common lease == previous_lease.checked_add(1), registry_generation
  > previous_registry_generation, previous/current exact host identities differ,
  and both host PIDs differ from retained guardian PID. Same host PID with different
  creation FILETIME is allowed as a possible reuse across terminated host epochs;
- takeover authorization whose host/lease does not equal the payload's exact
  previous_host/previous_lease. The authorization's generation/op/hash must be valid;
- Started with suspend count other than one; Failed with zero error.

Deferred G2b checks are explicitly not silently implemented in G2a: exact preceding
hash, record order/duplicate conflicts, matching historical launch operation and
host, binding establishment, known child versus current host, current registry
fence, operation result correspondence, once-only resume across leases, settlement
request/observation/consumption correspondence, proof hash and journal sequence
references, sealed phase, historical/mixed-run authority, and sticky conflicts.
A shape literal may be individually valid without forming a valid transcript.
No operation-ID uniqueness or cross-record liveness is inferred by the DTO.
For later cross-record validation, settlement_hash names the complete accepted
SettlementObserved record digest (including context/operation/predecessor/lease),
and settlement_sequence names that committed journal row. It is not a second,
context-free hash of only the nested proof object. G2a validates scalar shape only.

## Strict deserialization

Each raw map, context, binding, payload, outcome and proof rejects unknown and
repeated fields and missing required fields; internally tagged enums reject unknown
variants. Every path ends in the same validating constructor used by direct calls.
No derive Deserialize on private invariant-bearing fields without try_from/validator.

For the mandatory nullable fields, use a private non-Option presence type, e.g.
untagged `RequiredNullable<T> { Null(()), Value(T) }` that buffers through
`deserialize_any`, or a small explicit outer map-presence visitor. Its decoded
value maps to Option only after successful raw presence validation. Do NOT use
Option<Option<T>>, serde default, or a newtype whose Deserialize merely delegates
to Option<T>: serde's missing-field deserializer can synthesize None through that
path. Literal missing-vs-explicit-null tests decide correctness, including both
fields omitted separately and together. The chosen wrapper must itself have a
negative missing-field test before it is trusted.

Use dedicated raw canonical hash/string ID adapters, not the general ContentHash
or ID serde implementation at new scalar fields. Existing nested G1 objects already
have their own strict adapters; retain them. Reject floats, negative values,
stringified numbers and overflow, including numeric tokens parsed from literal
JSON text (not only serde_json::Value, which can normalize evidence).
Validation errors are static typed categories and do not format untrusted payloads.
DTO bounds do not replace outer IPC/journal input size limits in later stages.

## Frozen canonical bytes (no JSON hashing)

All concatenation below is exact, big-endian, with no padding and no terminators
unless explicitly stated. Hash is SHA-256 over this complete preimage; return
ContentHash rather than reinterpreting it as an authenticated receipt.

1. ASCII `surge.windows-guardian-ledger.v1` followed by ONE NUL byte.
2. Inner version u32 BE (1).
3. Context: run raw ULID 16 bytes, invocation raw ULID 16, occurrence raw ULID 16,
   then authority.
4. Operation raw 16 bytes.
5. Predecessor: byte 0 for null, or byte 1 + raw SHA-256 32 bytes.
6. Lease: byte 0 for null, or byte 1 + lease u64 BE.
7. Body: one-byte tag in the table above, then its listed fields in listed order.

Nested encodings:

- Authority = installation raw 32 + executable digest raw 32 + protocol u32 BE.
- Process = PID u32 BE + creation FILETIME u64 BE.
- Binding = authority + occurrence raw 16 + guardian process + endpoint string.
- Endpoint string = byte length u32 BE + exact ASCII bytes (always 106 after
  validation). Length is bytes, not chars. No NUL, Unicode normalization or casing
  transformation during encoding; derivation already happened at construction.
- Container = kind byte 1 + container version u32 BE=1 + binding reconstructed from
  G1 identity getters + child process + coverage byte 1 (GroupOnly).
- Every positive generation/lease and journal sequence = u64 BE; journal sequence
  is constructor-capped at i64::MAX despite this canonical field width.
- Operation reference = 16 raw bytes; any hash = 32 raw bytes without sha256: text.
- Outcome = byte 1 + previous suspend count u32 BE=1; or byte 2 + nonzero Win32
  error u32 BE; or byte 3 alone for uncertain.
- Takeover authorization = byte 1 + host process + lease u64 BE + observation
  generation u64 BE; or byte 2 + host process + lease u64 BE + operation16 + hash32.
- Settlement proof = byte 1 + binding + barrier u64 BE + query u64 BE; or byte 2 +
  container + barrier u64 BE + query u64 BE.

No sorting, serde discriminant ordinals, platform usize, native endianness, Display
ULID prefixes, hash-prefix text, or JSON serialization enters the preimage.
All variable lengths are statically bounded by validated shape. Keep the encoder
small and direct; do not create a generic serialization framework.

## Independent literal fixtures and tests

Existing independent fixture source:
/tmp/surge-g2a-independent-literal-oracles.md. Its three literal JSON shapes and
complete hex preimages predate the implementation and must be copied unchanged
into test constants/resources, not generated by the new encoder. Recomputed using
Python hashlib during this plan, all declared lengths and digests match:

| Shape | Preimage bytes | SHA-256 |
|---|---:|---|
| lease_taken_over | 495 | 2d6758e4ddfd72923f0cbc927d65342a452c81668e4e258f451181fc5f97aea5 |
| settlement_observed / bound_job_empty | 451 | 30328b867cab398d9cea2e3c64517789681e334c47b2b8b9abd891d95c57de7d |
| settlement_observed / child_job_empty | 469 | 83d1b0d41277b85d7446f0e7462f5887d2ef347ca1f13f66178b40d67daec846 |
| launch_intent genesis | 192 | f0d2842157aefb73553b44878cd654aeb0382fd0084bd855c40e7709e576fe8a |

Genesis literal identity: run ULID1, invocation ULID2, occurrence ULID3,
installation integer1 encoded32, zero executable digest, protocol1, operation
integer1 encoded16, predecessor null, lease null, host PID4/FILETIME5,
registry_generation6. Its complete preimage is frozen separately at
/tmp/surge-g2a-genesis.hex and below. This is another shape oracle, not native evidence.

```text
73757267652e77696e646f77732d677561726469616e2d6c65646765722e763100000000010000000000000000000000000000000100000000000000000000000000000002000000000000000000000000000000030000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000010000010000000400000000000000050000000000000006
```

Test inventory before implementation:

1. Decode each literal, assert exact named getters/typed variant, serialize back
   to the exact JSON object, assert canonical_bytes equals literal hex bytes, and
   hash equals literal digest. Assert length separately. JSON whitespace and key
   order changes preserve preimage/hash.
2. Add independent literal valid shapes for the other body/outcome/authorization
   branches before coding their encoder branches. At minimum hard-code expected
   body tag and ordered field suffix bytes for all ten tags; obtain full independent
   digests for cancellation, bound, child, both resume observations and consumption.
   Do not use the production encoder to compute expected vectors.
3. Mandatory-nullable omission/null/type tests as above, duplicate-field literal
   strings, unknown fields at every nesting level, unknown kind/version/outcome,
   absent required body fields. Include explicit positive controls to ensure each
   mutated fixture starts from a shape that decodes successfully.
4. Boundary matrix: nil/canonical ID aliases (including alphabetic ULID), zero and
   uppercase operation/install IDs, bare/uppercase hash aliases, positive integer
   extremes, overflow numeric literals, floats/stringified numbers. Lease u64::MAX
   can exist outside takeover; previous_lease u64::MAX cannot advance. Journal
   sequence i64::MAX accepts; i64::MAX+1 and zero reject.
5. Exact binding: changed authority/occurrence/endpoint, host/guardian collisions,
   mismatched takeover old host/lease, skipped common lease, nonincreasing registry
   generation, same exact host identity. Same numeric host PID with different
   FILETIME remains a structurally valid takeover candidate.
6. Constructor parity tests bypass serde and try every expressible invalid state;
   typed payload construction cannot hide an invalid nested value. Ensure direct
   access cannot construct public enum payload fields that skip validation.
7. Golden mutation tests change one independently specified valid field at a time
   and require a changed digest/preimage; do not rely only on this sensitivity test
   (a wrong encoder also changes hashes). Outcome/proof discriminator separation
   and two differently framed fields must have distinct literal encodings.
8. Existing G1 literal bytes and strict parsing tests remain unchanged. No G2a DTO
   is convertible to WriterContainer or a runtime authority token.

Implementation order: add fixed fixture resources/test assertions first, preserve
first failing compiler/test output; implement validated scalar/binding/context
constructors and strict raw decoding; implement per-kind bodies and local record
checks; implement exact direct encoder; run focused G2a/G1 tests, strict full-feature
surge-core Clippy/fmt, then all surge-core tests/MSRV gate as coordinator permits.
Record which initial failures are missing implementation versus actual behavioral
RED. Do not claim a baseline runtime defect from a compile-only red.

Verification commands (only after exclusive Cargo token is granted):
CARGO_INCREMENTAL=0 cargo nextest run -p surge-core -E 'test(guardian_ledger)'
CARGO_INCREMENTAL=0 cargo nextest run -p surge-core -E 'test(guardian)'
CARGO_INCREMENTAL=0 cargo clippy --locked -p surge-core --all-targets --all-features -- -D warnings
CARGO_INCREMENTAL=0 cargo nextest run --locked -p surge-core --all-features
Use repository's pinned MSRV 1.96 scoped core check, not an inferred current toolchain.

Maintainer rationale: this finishes one pure dependency of accepted G2 without
changing historical journal semantics prematurely. Reuses all validated G1 objects,
adds only the missing childless binding and typed record vocabulary, and leaves
native origin, sequencing and authority explicitly to their real owners. Separate
spec compliance then API/security/quality review must validate constructor closure,
mandatory nullable fields and independent byte vectors before this slice is done.
