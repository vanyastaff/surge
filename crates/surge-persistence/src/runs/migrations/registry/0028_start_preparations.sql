-- Host-only provisional preparation, never an attempt or a Prepared acknowledgment.
CREATE TABLE work_item_start_preparations (
 item TEXT PRIMARY KEY REFERENCES work_items(id),
 token TEXT NOT NULL UNIQUE, operation TEXT NOT NULL, body_hash TEXT NOT NULL,
 snapshot TEXT NOT NULL, lock_identity TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('preparing','consumed','abandoned'))
);
