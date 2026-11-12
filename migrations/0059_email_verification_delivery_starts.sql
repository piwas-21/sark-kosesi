-- A start is local work custody. It cannot attest provider or mailbox state.
CREATE TABLE email_verification_delivery_starts (
 verifier bytea PRIMARY KEY REFERENCES email_verification_delivery_envelopes(verifier),
 key_id text NOT NULL REFERENCES email_verification_delivery_keys(key_id),
 nonce_counter bigint NOT NULL,
 key_fingerprint bytea NOT NULL CHECK(octet_length(key_fingerprint)=32),
 started_at bigint NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740991),
 transaction_id xid8 NOT NULL,
 FOREIGN KEY(key_id,nonce_counter) REFERENCES email_verification_delivery_nonces(key_id,nonce_counter)
);

CREATE FUNCTION email_verification_delivery_start_source(s email_verification_delivery_starts) RETURNS boolean
 LANGUAGE sql VOLATILE AS $$
 SELECT EXISTS(
  SELECT 1 FROM email_verification_challenges c
  JOIN email_verification_delivery_envelopes e ON e.verifier=c.verifier
  JOIN email_verification_delivery_nonces n ON n.key_id=e.key_id AND n.nonce_counter=e.nonce_counter AND n.verifier=c.verifier
  JOIN email_verification_delivery_keys k ON k.key_id=n.key_id
  WHERE c.verifier=s.verifier AND c.purpose='email-verification-v1'
   AND c.delivery_mode='envelope' AND c.consumed_at IS NULL AND c.consumed_xid IS NULL
   AND c.created_at<=s.started_at AND s.started_at<c.expires_at
   AND n.reserved_at BETWEEN k.registered_at AND c.created_at
   AND e.key_id=s.key_id AND e.nonce_counter=s.nonce_counter AND k.key_fingerprint=s.key_fingerprint
   AND e.envelope_version=1 AND octet_length(e.ciphertext)=48
   AND c.delivery_transaction_id=e.transaction_id AND e.transaction_id<>n.transaction_id
   AND e.transaction_id<>s.transaction_id AND n.transaction_id<>s.transaction_id
 );
$$;

CREATE FUNCTION guard_email_verification_delivery_start() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Verification delivery starts are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Verification delivery start requires read committed' USING ERRCODE='23514'; END IF;
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery start requires durable commit' USING ERRCODE='23514'; END IF;
 IF NEW.transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Verification delivery start transaction is assigned by the database' USING ERRCODE='23514'; END IF;
 -- Consumption updates this row. Read the complete source in a fresh statement after waiting.
 PERFORM 1 FROM email_verification_challenges WHERE verifier=NEW.verifier FOR UPDATE;
 NEW.transaction_id=pg_current_xact_id();
 IF NOT email_verification_delivery_start_source(NEW) THEN
  RAISE EXCEPTION 'Verification delivery start requires its exact live envelope' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_email_verification_delivery_start BEFORE INSERT OR UPDATE OR DELETE ON email_verification_delivery_starts
 FOR EACH ROW EXECUTE FUNCTION guard_email_verification_delivery_start();

CREATE FUNCTION require_email_verification_delivery_start() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery start requires durable commit' USING ERRCODE='23514'; END IF;
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR NOT email_verification_delivery_start_source(NEW) THEN
  RAISE EXCEPTION 'Verification delivery start requires its exact live envelope' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_email_verification_delivery_start AFTER INSERT ON email_verification_delivery_starts
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_email_verification_delivery_start();

-- Flushing the deferred check early cannot permit a same-transaction consumption.
CREATE FUNCTION seal_email_verification_delivery_start_source() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM email_verification_delivery_starts
  WHERE verifier=OLD.verifier AND transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Verification delivery start and challenge consumption require separate transactions' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER seal_email_verification_delivery_start_source BEFORE UPDATE ON email_verification_challenges
 FOR EACH ROW EXECUTE FUNCTION seal_email_verification_delivery_start_source();
