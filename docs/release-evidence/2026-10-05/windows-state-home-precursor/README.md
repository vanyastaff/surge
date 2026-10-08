# Windows state-home native RED precursor

This stage adds three existing-API tests and required dedicated-standard-user CI execution. Production home creation, SQLite ownership and private guards are unchanged. Independent spec/security reviews accepted final test hash `c194cb599e77b79b2f0ae0457dfde26e98daa6dffcf4430a5194283709da0b81` and script hash `ad405b37728c5a598400126b3ccbc3ccfa026275d760b780933d095b68adaefe`.

The positive ACL oracle inspects actual TokenUser, owner, protected DACL, exact single-user full grant and exact inheritance shapes after Storage opens a missing child home under token-resolved LocalAppData. This proves resulting permissions only; creation-time protection before SQL remains an independent full-backend requirement.

The lifetime oracle checks out every actual registry pool slot, verifies its original canonical database identity, replaces each connection with a real in-memory SQLite connection and explicitly closes the original. Exact empty in-memory paths and full pool occupancy prove no SQLite disk connection remains or can replenish. It then drops Storage while retaining the real pool and slots. Rename must refuse until all actual owners drop, then succeed within a bounded deadline without ACL changes. This isolates the permanent manager fence from SQLite's incidental file-sharing fence; it does not establish two-connection database compatibility.

The negative oracle first independently proves an outsider full-access allow ACE in the deliberately unsafe isolated PublicRoot child. Storage must refuse while owner/DACL/sentinel bytes and absence of db/runs remain unchanged. PublicRoot is not used as a trusted private home ancestor.

The existing NTFS primitive and all three new ignored tests execute explicitly in the mandatory separate standard-user CI step; their success receipts are required. No existing test is disabled. The current production implementation is expected to fail the new guarantees; native behavioral RED has not yet run.

Local checks: `CARGO_INCREMENTAL=0 cargo clippy --locked -p surge-persistence --lib --tests -- -D warnings` passed; `CARGO_INCREMENTAL=0 cargo fmt -p surge-persistence --check` and `git diff --check` passed. Windows-only code is cfg-excluded on macOS: these checks are not native compilation or execution evidence. Full local Windows persistence compilation is blocked by the unavailable MSVC SDK, not bypassed with headers or stubs. `manifest.json` binds source and original log bytes; compressed logs retain actual commands/results.
