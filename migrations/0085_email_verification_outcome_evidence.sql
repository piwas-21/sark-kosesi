-- Existing callbacks retain the original classification and legacy provenance.
-- New receipts require a commitment to the ciphertext authenticated by their opener.
ALTER TABLE email_verification_delivery_outcomes
 ADD COLUMN evidence_version smallint NOT NULL DEFAULT 0,
 ADD COLUMN ciphertext_sha256 bytea,
 ADD CONSTRAINT email_verification_delivery_outcomes_evidence_shape CHECK (
  (evidence_version=0 AND ciphertext_sha256 IS NULL)
  OR (evidence_version=1 AND ciphertext_sha256 IS NOT NULL AND octet_length(ciphertext_sha256)=32)
 );
ALTER TABLE email_verification_delivery_outcomes ALTER COLUMN evidence_version SET DEFAULT 1;

CREATE OR REPLACE FUNCTION guard_email_verification_delivery_outcome() RETURNS trigger
 LANGUAGE plpgsql VOLATILE AS $$
DECLARE original email_verification_delivery_starts%ROWTYPE; ciphertext_digest bytea;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Verification delivery outcomes are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed'
   OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery outcome requires durable read committed' USING ERRCODE='23514'; END IF;
 IF NEW.transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Verification delivery outcome transaction is assigned by the database' USING ERRCODE='23514'; END IF;
 IF NEW.evidence_version IS DISTINCT FROM 1 OR NEW.ciphertext_sha256 IS NULL THEN
  RAISE EXCEPTION 'New verification outcomes require authenticated envelope evidence' USING ERRCODE='23514'; END IF;
 SELECT * INTO original FROM email_verification_delivery_starts WHERE verifier=NEW.verifier FOR SHARE;
 SELECT CASE WHEN envelope_version=1 AND octet_length(ciphertext)=48 THEN sha256(ciphertext) END
  INTO ciphertext_digest FROM email_verification_delivery_envelopes
  WHERE verifier=NEW.verifier AND key_id=original.key_id AND nonce_counter=original.nonce_counter FOR SHARE;
 NEW.transaction_id=pg_current_xact_id();
 IF original.verifier IS NULL OR original.transaction_id IS DISTINCT FROM NEW.start_transaction_id
   OR original.transaction_id=NEW.transaction_id OR NEW.observed_at<original.started_at
   OR ciphertext_digest IS DISTINCT FROM NEW.ciphertext_sha256 THEN
  RAISE EXCEPTION 'Verification delivery outcome requires its prior start and envelope' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION require_email_verification_delivery_outcome() RETURNS trigger
 LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on'
   OR NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
   OR NEW.evidence_version IS DISTINCT FROM 1 OR NEW.ciphertext_sha256 IS NULL
   OR NOT EXISTS(SELECT 1 FROM email_verification_delivery_starts s
    JOIN email_verification_delivery_envelopes e
     ON e.verifier=s.verifier AND e.key_id=s.key_id AND e.nonce_counter=s.nonce_counter
    JOIN email_verification_delivery_outcomes o ON o.verifier=s.verifier
    WHERE s.verifier=NEW.verifier AND s.transaction_id=NEW.start_transaction_id
     AND s.transaction_id<>NEW.transaction_id AND s.started_at<=NEW.observed_at
     AND o.transaction_id=NEW.transaction_id AND o.start_transaction_id=NEW.start_transaction_id
     AND o.evidence_version=NEW.evidence_version AND o.ciphertext_sha256=NEW.ciphertext_sha256
     AND CASE WHEN e.envelope_version=1 AND octet_length(e.ciphertext)=48
      THEN sha256(e.ciphertext)=NEW.ciphertext_sha256 ELSE false END) THEN
  RAISE EXCEPTION 'Verification delivery outcome requires its prior durable start and envelope' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
