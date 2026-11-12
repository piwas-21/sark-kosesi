-- A transport classification is evidence of a callback result, not delivery.
CREATE TABLE email_verification_delivery_outcomes (
 verifier bytea PRIMARY KEY REFERENCES email_verification_delivery_starts(verifier),
 start_transaction_id xid8 NOT NULL,
 outcome text NOT NULL CHECK(outcome IN ('accepted','explicit_rejection','uncertain')),
 observed_at bigint NOT NULL CHECK(observed_at BETWEEN 0 AND 9007199254740991),
 transaction_id xid8 NOT NULL
);

CREATE FUNCTION guard_email_verification_delivery_outcome() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE original email_verification_delivery_starts%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Verification delivery outcomes are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed'
   OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Verification delivery outcome requires durable read committed' USING ERRCODE='23514'; END IF;
 IF NEW.transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Verification delivery outcome transaction is assigned by the database' USING ERRCODE='23514'; END IF;
 SELECT * INTO original FROM email_verification_delivery_starts WHERE verifier=NEW.verifier FOR SHARE;
 NEW.transaction_id=pg_current_xact_id();
 IF NOT FOUND OR original.transaction_id IS DISTINCT FROM NEW.start_transaction_id
   OR original.transaction_id=NEW.transaction_id OR NEW.observed_at<original.started_at THEN
  RAISE EXCEPTION 'Verification delivery outcome requires its prior start' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_email_verification_delivery_outcome BEFORE INSERT OR UPDATE OR DELETE ON email_verification_delivery_outcomes
 FOR EACH ROW EXECUTE FUNCTION guard_email_verification_delivery_outcome();

CREATE FUNCTION require_email_verification_delivery_outcome() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on'
   OR NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
   OR NOT EXISTS(SELECT 1 FROM email_verification_delivery_starts s WHERE s.verifier=NEW.verifier
      AND s.transaction_id=NEW.start_transaction_id AND s.transaction_id<>NEW.transaction_id
      AND s.started_at<=NEW.observed_at) THEN
  RAISE EXCEPTION 'Verification delivery outcome requires its prior durable start' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_email_verification_delivery_outcome AFTER INSERT ON email_verification_delivery_outcomes
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_email_verification_delivery_outcome();
