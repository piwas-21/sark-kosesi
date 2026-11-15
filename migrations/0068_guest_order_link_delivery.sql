-- Private guest-link ciphertext custody. Nonces commit before encryption and
-- remain burned when the later proof/envelope transaction fails.
CREATE TABLE guest_link_delivery_keys (
 key_id text PRIMARY KEY CHECK(octet_length(key_id) BETWEEN 1 AND 64 AND key_id ~ '^[a-zA-Z0-9][a-zA-Z0-9_-]*$'),
 key_fingerprint bytea UNIQUE NOT NULL CHECK(octet_length(key_fingerprint)=32),
 registered_at bigint NOT NULL CHECK(registered_at BETWEEN 0 AND 9007199254740991)
);
CREATE FUNCTION guard_guest_link_delivery_key() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 RAISE EXCEPTION 'Guest link delivery keys are immutable' USING ERRCODE='23514';
END $$;
CREATE TRIGGER guard_guest_link_delivery_key BEFORE UPDATE OR DELETE ON guest_link_delivery_keys
 FOR EACH ROW EXECUTE FUNCTION guard_guest_link_delivery_key();

CREATE TABLE guest_link_delivery_nonces (
 key_id text NOT NULL REFERENCES guest_link_delivery_keys(key_id),
 nonce_counter bigint NOT NULL CHECK(nonce_counter BETWEEN 1 AND 1000000),
 verifier bytea UNIQUE NOT NULL CHECK(octet_length(verifier)=32),
 reserved_at bigint NOT NULL CHECK(reserved_at BETWEEN 0 AND 9007199254740991),
 transaction_id xid8 NOT NULL,
 PRIMARY KEY(key_id,nonce_counter)
);
CREATE FUNCTION guard_guest_link_delivery_nonce() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE key_registered bigint; previous_counter bigint; previous_at bigint;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Guest link nonces are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Guest link nonce requires durable read committed' USING ERRCODE='23514'; END IF;
 SELECT registered_at INTO key_registered FROM guest_link_delivery_keys WHERE key_id=NEW.key_id FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Guest link nonce requires key' USING ERRCODE='23514'; END IF;
 SELECT nonce_counter,reserved_at INTO previous_counter,previous_at FROM guest_link_delivery_nonces WHERE key_id=NEW.key_id ORDER BY nonce_counter DESC LIMIT 1;
 IF NEW.reserved_at<key_registered OR NEW.reserved_at<previous_at OR coalesce(previous_counter,0)>=1000000 THEN
  RAISE EXCEPTION 'Invalid guest link nonce allocation' USING ERRCODE='23514'; END IF;
 NEW.nonce_counter=coalesce(previous_counter,0)+1;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END $$;
CREATE TRIGGER guard_guest_link_delivery_nonce BEFORE INSERT OR UPDATE OR DELETE ON guest_link_delivery_nonces
 FOR EACH ROW EXECUTE FUNCTION guard_guest_link_delivery_nonce();
CREATE FUNCTION require_guest_link_nonce_durable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Guest link nonce requires durable commit' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_guest_link_nonce_durable AFTER INSERT ON guest_link_delivery_nonces
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_guest_link_nonce_durable();

ALTER TABLE guest_order_link_proofs ADD COLUMN delivery_mode text NOT NULL DEFAULT 'direct',
 ADD COLUMN delivery_transaction_id xid8,
 ADD CONSTRAINT guest_link_delivery_mode_check CHECK (
  (delivery_mode='direct' AND delivery_transaction_id IS NULL) OR
  (delivery_mode='envelope' AND delivery_transaction_id IS NOT NULL));
CREATE FUNCTION guard_guest_link_delivery_mode() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  IF NEW.delivery_transaction_id IS NOT NULL OR NEW.delivery_mode NOT IN ('direct','envelope') THEN
   RAISE EXCEPTION 'Invalid guest link delivery mode' USING ERRCODE='23514'; END IF;
  IF NEW.delivery_mode='envelope' THEN NEW.delivery_transaction_id=pg_current_xact_id(); END IF;
 ELSIF (NEW.delivery_mode,NEW.delivery_transaction_id) IS DISTINCT FROM (OLD.delivery_mode,OLD.delivery_transaction_id) THEN
  RAISE EXCEPTION 'Guest link delivery mode is immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER guard_guest_link_delivery_mode BEFORE INSERT OR UPDATE ON guest_order_link_proofs
 FOR EACH ROW EXECUTE FUNCTION guard_guest_link_delivery_mode();

CREATE TABLE guest_link_delivery_envelopes (
 verifier bytea PRIMARY KEY REFERENCES guest_order_link_proofs(verifier),
 key_id text NOT NULL,
 nonce_counter bigint NOT NULL,
 envelope_version smallint NOT NULL CHECK(envelope_version=1),
 ciphertext bytea NOT NULL CHECK(octet_length(ciphertext)=48),
 transaction_id xid8 NOT NULL,
 UNIQUE(key_id,nonce_counter),
 FOREIGN KEY(key_id,nonce_counter) REFERENCES guest_link_delivery_nonces(key_id,nonce_counter)
);
CREATE FUNCTION guard_guest_link_delivery_envelope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Guest link envelopes are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Guest link envelope requires durable read committed' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM guest_order_link_proofs p JOIN guest_link_delivery_nonces n ON n.verifier=p.verifier
  WHERE p.verifier=NEW.verifier AND p.delivery_mode='envelope' AND p.delivery_transaction_id=pg_current_xact_id()
   AND n.key_id=NEW.key_id AND n.nonce_counter=NEW.nonce_counter AND n.transaction_id<>pg_current_xact_id() AND p.created_at>=n.reserved_at) THEN
  RAISE EXCEPTION 'Guest link envelope requires proof and prior nonce' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END $$;
CREATE TRIGGER guard_guest_link_delivery_envelope BEFORE INSERT OR UPDATE OR DELETE ON guest_link_delivery_envelopes
 FOR EACH ROW EXECUTE FUNCTION guard_guest_link_delivery_envelope();
CREATE FUNCTION require_guest_link_proof_envelope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.delivery_mode='envelope' AND NOT EXISTS(SELECT 1 FROM guest_link_delivery_envelopes e
  WHERE e.verifier=NEW.verifier AND e.transaction_id=NEW.delivery_transaction_id AND e.transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Guest link proof requires envelope' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_guest_link_proof_envelope AFTER INSERT ON guest_order_link_proofs
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_guest_link_proof_envelope();
CREATE FUNCTION require_guest_link_envelope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' OR NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR NOT EXISTS(
  SELECT 1 FROM guest_order_link_proofs p JOIN guest_link_delivery_nonces n ON n.verifier=p.verifier
  WHERE p.verifier=NEW.verifier AND p.delivery_mode='envelope' AND p.delivery_transaction_id=NEW.transaction_id
   AND n.key_id=NEW.key_id AND n.nonce_counter=NEW.nonce_counter AND n.transaction_id<>NEW.transaction_id AND p.created_at>=n.reserved_at) THEN
  RAISE EXCEPTION 'Guest link envelope requires proof and prior nonce' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_guest_link_envelope AFTER INSERT ON guest_link_delivery_envelopes
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_guest_link_envelope();
