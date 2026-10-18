-- External write-once private storage identity. Losing the private directory
-- never removes the key-creation decision or grants permission to establish again.
CREATE TABLE owned_flow_private_establishment (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    schema_version INTEGER NOT NULL CHECK(schema_version=1),
    nonce TEXT NOT NULL UNIQUE CHECK(length(nonce)>0 AND length(nonce)<=128),
    state TEXT NOT NULL CHECK(state IN ('Establishing','Established')),
    lock_identity TEXT NOT NULL CHECK(length(lock_identity)>0 AND length(lock_identity)<=65536),
    namespace_identity TEXT CHECK(length(namespace_identity)>0 AND length(namespace_identity)<=65536),
    key_identity BLOB CHECK(length(key_identity) IN (16,24)),
    key_digest BLOB CHECK(length(key_digest)=32),
    marker_identity BLOB CHECK(length(marker_identity) IN (16,24)),
    CHECK(state='Establishing' OR (namespace_identity IS NOT NULL AND key_identity IS NOT NULL AND key_digest IS NOT NULL AND marker_identity IS NOT NULL)),
    CHECK((key_identity IS NULL)=(key_digest IS NULL))
);
CREATE TRIGGER owned_flow_private_establishment_immutable
BEFORE UPDATE ON owned_flow_private_establishment
WHEN NEW.singleton IS NOT OLD.singleton OR NEW.schema_version IS NOT OLD.schema_version
 OR NEW.nonce IS NOT OLD.nonce OR NEW.lock_identity IS NOT OLD.lock_identity
 OR (OLD.namespace_identity IS NOT NULL AND NEW.namespace_identity IS NOT OLD.namespace_identity)
 OR (OLD.key_identity IS NOT NULL AND NEW.key_identity IS NOT OLD.key_identity)
 OR (OLD.key_digest IS NOT NULL AND NEW.key_digest IS NOT OLD.key_digest)
 OR (OLD.marker_identity IS NOT NULL AND NEW.marker_identity IS NOT OLD.marker_identity)
 OR OLD.state='Established'
BEGIN SELECT RAISE(ABORT,'immutable private establishment'); END;
CREATE TRIGGER owned_flow_private_establishment_no_delete
BEFORE DELETE ON owned_flow_private_establishment
BEGIN SELECT RAISE(ABORT,'permanent private establishment'); END;
