-- Private encrypted verification custody. Key bytes and plaintext bearers are
-- never stored here. A committed nonce allocation remains burned even when a
-- later challenge/envelope transaction fails.
CREATE TABLE email_verification_delivery_keys (
 key_id text PRIMARY KEY CHECK(octet_length(key_id) BETWEEN 1 AND 64 AND key_id ~ '^[a-zA-Z0-9][a-zA-Z0-9_-]*$'),
 key_fingerprint bytea UNIQUE NOT NULL CHECK(octet_length(key_fingerprint)=32),
 registered_at bigint NOT NULL CHECK(registered_at BETWEEN 0 AND 9007199254740991)
);
CREATE FUNCTION guard_email_verification_delivery_key() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 RAISE EXCEPTION 'Verification delivery keys are immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER guard_email_verification_delivery_key BEFORE UPDATE OR DELETE ON email_verification_delivery_keys
 FOR EACH ROW EXECUTE FUNCTION guard_email_verification_delivery_key();

CREATE TABLE email_verification_delivery_nonces (
 key_id text NOT NULL REFERENCES email_verification_delivery_keys(key_id),
 nonce_counter bigint NOT NULL CHECK(nonce_counter BETWEEN 1 AND 1000000),
 verifier bytea UNIQUE NOT NULL CHECK(octet_length(verifier)=32),
 reserved_at bigint NOT NULL CHECK(reserved_at BETWEEN 0 AND 9007199254740991),
 transaction_id xid8 NOT NULL,
 PRIMARY KEY(key_id,nonce_counter)
);

-- Direct UPDATE-mode row locking avoids upgrades. The predecessor query is a
-- separate READ COMMITTED statement after any wait and uses the primary key.
CREATE FUNCTION guard_email_verification_delivery_nonce() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE key_registered bigint; previous_counter bigint; previous_at bigint;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Verification delivery nonces are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Verification delivery nonce requires read committed' USING ERRCODE='23514'; END IF;
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery nonce requires durable commit' USING ERRCODE='23514'; END IF;
 SELECT registered_at INTO key_registered FROM email_verification_delivery_keys WHERE key_id=NEW.key_id FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Verification delivery nonce requires key' USING ERRCODE='23514'; END IF;
 SELECT nonce_counter,reserved_at INTO previous_counter,previous_at
  FROM email_verification_delivery_nonces WHERE key_id=NEW.key_id ORDER BY nonce_counter DESC LIMIT 1;
 IF NEW.reserved_at<key_registered OR NEW.reserved_at<previous_at THEN
  RAISE EXCEPTION 'Verification delivery nonce time must not regress' USING ERRCODE='23514'; END IF;
 IF coalesce(previous_counter,0)>=1000000 THEN
  RAISE EXCEPTION 'Verification delivery nonce limit reached' USING ERRCODE='23514'; END IF;
 NEW.nonce_counter=coalesce(previous_counter,0)+1;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_email_verification_delivery_nonce BEFORE INSERT OR UPDATE OR DELETE ON email_verification_delivery_nonces
 FOR EACH ROW EXECUTE FUNCTION guard_email_verification_delivery_nonce();

CREATE FUNCTION require_email_verification_nonce_durable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
  OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery nonce requires durable commit' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_email_verification_nonce_durable AFTER INSERT ON email_verification_delivery_nonces
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_email_verification_nonce_durable();

-- Existing direct challenges retain their old state and consumption behavior.
ALTER TABLE email_verification_challenges
 ADD COLUMN delivery_mode text NOT NULL DEFAULT 'direct',
 ADD COLUMN delivery_transaction_id xid8,
 ADD CONSTRAINT email_verification_delivery_mode_check CHECK (
  (delivery_mode='direct' AND delivery_transaction_id IS NULL) OR
  (delivery_mode='envelope' AND delivery_transaction_id IS NOT NULL));

-- Preserve the 0035 one-use consumption guard and extend the immutable header.
CREATE OR REPLACE FUNCTION guard_email_verification_challenge() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.consumed_at IS NOT NULL OR NEW.consumed_xid IS NOT NULL
            OR NEW.delivery_transaction_id IS NOT NULL
            OR NEW.delivery_mode NOT IN ('direct','envelope') THEN
            RAISE EXCEPTION 'Invalid verification challenge insertion' USING ERRCODE='23514';
        END IF;
        IF NEW.delivery_mode='envelope' THEN
            NEW.delivery_transaction_id := pg_current_xact_id();
        END IF;
        RETURN NEW;
    END IF;
    IF (NEW.verifier,NEW.purpose,NEW.email_identity,NEW.created_at,NEW.expires_at,
        NEW.delivery_mode,NEW.delivery_transaction_id)
        IS DISTINCT FROM (OLD.verifier,OLD.purpose,OLD.email_identity,OLD.created_at,OLD.expires_at,
        OLD.delivery_mode,OLD.delivery_transaction_id)
        OR OLD.consumed_at IS NOT NULL OR OLD.consumed_xid IS NOT NULL
        OR NEW.consumed_at IS NULL OR NEW.consumed_xid IS NOT NULL THEN
        RAISE EXCEPTION 'Invalid verification challenge transition' USING ERRCODE='23514';
    END IF;
    NEW.consumed_xid := pg_current_xact_id();
    RETURN NEW;
END;
$$;

CREATE TABLE email_verification_delivery_envelopes (
 verifier bytea PRIMARY KEY REFERENCES email_verification_challenges(verifier),
 key_id text NOT NULL,
 nonce_counter bigint NOT NULL,
 envelope_version smallint NOT NULL CHECK(envelope_version=1),
 ciphertext bytea NOT NULL CHECK(octet_length(ciphertext)=48),
 transaction_id xid8 NOT NULL,
 UNIQUE(key_id,nonce_counter),
 FOREIGN KEY(key_id,nonce_counter) REFERENCES email_verification_delivery_nonces(key_id,nonce_counter)
);

CREATE FUNCTION guard_email_verification_delivery_envelope() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Verification delivery envelopes are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Verification delivery envelope requires read committed' USING ERRCODE='23514'; END IF;
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery envelope requires durable commit' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM email_verification_challenges c
  JOIN email_verification_delivery_nonces n ON n.verifier=c.verifier
  WHERE c.verifier=NEW.verifier AND c.delivery_mode='envelope'
   AND c.delivery_transaction_id=pg_current_xact_id()
   AND n.key_id=NEW.key_id AND n.nonce_counter=NEW.nonce_counter
   AND n.transaction_id<>pg_current_xact_id() AND c.created_at>=n.reserved_at) THEN
  RAISE EXCEPTION 'Verification delivery envelope requires exact challenge and prior nonce' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_email_verification_delivery_envelope BEFORE INSERT OR UPDATE OR DELETE ON email_verification_delivery_envelopes
 FOR EACH ROW EXECUTE FUNCTION guard_email_verification_delivery_envelope();

CREATE FUNCTION require_email_verification_challenge_envelope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.delivery_mode='envelope' AND NOT EXISTS(SELECT 1 FROM email_verification_delivery_envelopes e
  WHERE e.verifier=NEW.verifier AND e.transaction_id=NEW.delivery_transaction_id
   AND e.transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Verification challenge requires its same-transaction envelope' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_email_verification_challenge_envelope AFTER INSERT ON email_verification_challenges
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_email_verification_challenge_envelope();

CREATE FUNCTION require_email_verification_envelope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery envelope requires durable commit' USING ERRCODE='23514'; END IF;
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR NOT EXISTS(
  SELECT 1 FROM email_verification_challenges c JOIN email_verification_delivery_nonces n ON n.verifier=c.verifier
  WHERE c.verifier=NEW.verifier AND c.delivery_mode='envelope'
   AND c.delivery_transaction_id=NEW.transaction_id
   AND n.key_id=NEW.key_id AND n.nonce_counter=NEW.nonce_counter
   AND n.transaction_id<>NEW.transaction_id AND c.created_at>=n.reserved_at) THEN
  RAISE EXCEPTION 'Verification envelope requires its same-transaction challenge and prior nonce' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_email_verification_envelope AFTER INSERT ON email_verification_delivery_envelopes
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_email_verification_envelope();
