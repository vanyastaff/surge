-- Installing the schema does not certify historical normalized authority.
-- Exclusive owned replay advances this marker in the reset/replay transaction.
CREATE TABLE projection_semantics (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    version INTEGER NOT NULL CHECK(version>=0)
);
INSERT INTO projection_semantics(singleton,version) VALUES(1,0);
