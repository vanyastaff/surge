-- Authority audit survives usable run deletion; never backfill successful receipts.
CREATE TABLE run_host_openings (
    epoch INTEGER PRIMARY KEY AUTOINCREMENT,
    opening_id TEXT NOT NULL UNIQUE,
    run_id TEXT NOT NULL,
    owner_mode TEXT NOT NULL CHECK(owner_mode IN ('JournalOnly','Engine','Revalidation')),
    state TEXT NOT NULL CHECK(state IN ('Opening','JoinedWriter','ConfirmedExecution','Interrupted','Failed')),
    predecessor TEXT REFERENCES run_host_openings(opening_id),
    identity_version INTEGER NOT NULL CHECK(identity_version=1),
    route_identity BLOB NOT NULL CHECK(length(route_identity) IN (49,73)),
    database_identity BLOB CHECK(length(database_identity) IN (16,24)),
    unresolved_engine_opening TEXT REFERENCES run_host_openings(opening_id),
    last_confirmed_engine_receipt TEXT,
    diagnostic TEXT CHECK(length(diagnostic)<=4096)
);
CREATE INDEX run_host_openings_latest ON run_host_openings(run_id,epoch DESC);
CREATE TABLE run_host_settlements (
    receipt_id TEXT PRIMARY KEY,
    opening_id TEXT NOT NULL UNIQUE REFERENCES run_host_openings(opening_id),
    run_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    scope_version INTEGER NOT NULL CHECK(scope_version=1),
    provenance TEXT NOT NULL CHECK(provenance IN ('JoinedExecution','ExplicitHistoryRevalidation')),
    journal_identity BLOB NOT NULL CHECK(length(journal_identity) IN (65,97)),
    sealed_sequence INTEGER NOT NULL CHECK(sealed_sequence>=0),
    prefix_digest BLOB NOT NULL CHECK(length(prefix_digest)=32),
    terminal_sequence INTEGER,
    terminal_kind TEXT,
    terminal_schema_version INTEGER,
    terminal_digest BLOB,
    payload BLOB NOT NULL CHECK(length(payload)<=65536),
    CHECK((terminal_sequence IS NULL AND terminal_kind IS NULL AND terminal_schema_version IS NULL AND terminal_digest IS NULL)
       OR (terminal_sequence IS NOT NULL AND terminal_kind IS NOT NULL AND terminal_schema_version IS NOT NULL AND terminal_digest IS NOT NULL
           AND typeof(terminal_sequence)='integer' AND terminal_sequence>0 AND terminal_sequence<=sealed_sequence
           AND typeof(terminal_schema_version)='integer' AND terminal_schema_version BETWEEN 0 AND 4294967295 AND length(terminal_digest)=32))
);
CREATE TRIGGER run_host_settlements_no_update BEFORE UPDATE ON run_host_settlements BEGIN SELECT RAISE(ABORT,'immutable host settlement'); END;
CREATE TRIGGER run_host_settlements_no_delete BEFORE DELETE ON run_host_settlements BEGIN SELECT RAISE(ABORT,'immutable host settlement'); END;
CREATE TABLE run_host_tombstones (
    run_id TEXT PRIMARY KEY,
    deletion_id TEXT NOT NULL UNIQUE,
    original_opening TEXT REFERENCES run_host_openings(opening_id),
    journal_identity BLOB NOT NULL,
    provenance TEXT NOT NULL CHECK(provenance='ExplicitStorageDelete'),
    inventory_version INTEGER NOT NULL CHECK(inventory_version=1),
    inventory_count INTEGER NOT NULL CHECK(inventory_count BETWEEN 1 AND 100000),
    inventory_bytes INTEGER NOT NULL CHECK(inventory_bytes BETWEEN 1 AND 67108864),
    inventory_digest BLOB NOT NULL CHECK(length(inventory_digest)=32),
    outside_parent BLOB NOT NULL CHECK(length(outside_parent)>0 AND length(outside_parent)<=65536),
    state TEXT NOT NULL CHECK(state IN ('DeletePending','Deleted'))
);
CREATE TRIGGER run_host_tombstones_immutable BEFORE UPDATE ON run_host_tombstones
WHEN NEW.run_id IS NOT OLD.run_id OR NEW.deletion_id IS NOT OLD.deletion_id OR NEW.original_opening IS NOT OLD.original_opening
OR NEW.journal_identity IS NOT OLD.journal_identity OR NEW.provenance IS NOT OLD.provenance OR NEW.inventory_version IS NOT OLD.inventory_version
OR NEW.inventory_count IS NOT OLD.inventory_count OR NEW.inventory_bytes IS NOT OLD.inventory_bytes OR NEW.inventory_digest IS NOT OLD.inventory_digest
OR NEW.outside_parent IS NOT OLD.outside_parent OR OLD.state='Deleted'
BEGIN SELECT RAISE(ABORT,'immutable deletion intent'); END;
CREATE TRIGGER run_host_tombstones_no_delete BEFORE DELETE ON run_host_tombstones BEGIN SELECT RAISE(ABORT,'permanent deletion intent'); END;
CREATE TABLE run_host_deletion_inventory (
    deletion_id TEXT NOT NULL REFERENCES run_host_tombstones(deletion_id),
    relative_path BLOB NOT NULL,
    encoded_entry BLOB NOT NULL CHECK(length(encoded_entry)>0 AND length(encoded_entry)<=67108864),
    PRIMARY KEY(deletion_id,relative_path)
);
CREATE TRIGGER run_host_inventory_no_update BEFORE UPDATE ON run_host_deletion_inventory BEGIN SELECT RAISE(ABORT,'immutable deletion inventory'); END;
CREATE TRIGGER run_host_inventory_no_delete BEFORE DELETE ON run_host_deletion_inventory BEGIN SELECT RAISE(ABORT,'immutable deletion inventory'); END;
