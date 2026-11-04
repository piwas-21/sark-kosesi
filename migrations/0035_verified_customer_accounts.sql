-- Existing consumed rows retain NULL and cannot become account evidence. The
-- trigger, never the caller, binds a newly consumed challenge to this transaction.
ALTER TABLE email_verification_challenges ADD COLUMN consumed_xid xid8;
CREATE OR REPLACE FUNCTION guard_email_verification_challenge() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.consumed_at IS NOT NULL OR NEW.consumed_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid verification challenge insertion' USING ERRCODE='23514';
        END IF;
        RETURN NEW;
    END IF;
    IF (NEW.verifier,NEW.purpose,NEW.email_identity,NEW.created_at,NEW.expires_at)
        IS DISTINCT FROM (OLD.verifier,OLD.purpose,OLD.email_identity,OLD.created_at,OLD.expires_at)
        OR OLD.consumed_at IS NOT NULL OR OLD.consumed_xid IS NOT NULL
        OR NEW.consumed_at IS NULL OR NEW.consumed_xid IS NOT NULL THEN
        RAISE EXCEPTION 'Invalid verification challenge transition' USING ERRCODE='23514';
    END IF;
    NEW.consumed_xid := pg_current_xact_id();
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_email_verification_insert BEFORE INSERT ON email_verification_challenges
    FOR EACH ROW EXECUTE FUNCTION guard_email_verification_challenge();

-- Private verified identity custody; no login, session or guest authority.
CREATE TABLE verified_customer_accounts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email_identity text NOT NULL UNIQUE CHECK (octet_length(email_identity) BETWEEN 3 AND 254),
    verification_verifier bytea NOT NULL UNIQUE REFERENCES email_verification_challenges(verifier),
    verified_at bigint NOT NULL CHECK (verified_at >= 0)
);
CREATE FUNCTION guard_verified_customer_account() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        RAISE EXCEPTION 'Verified customer accounts are immutable' USING ERRCODE='23514';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM email_verification_challenges
        WHERE verifier=NEW.verification_verifier AND email_identity=NEW.email_identity
        AND consumed_at=NEW.verified_at AND consumed_xid=pg_current_xact_id()) THEN
        RAISE EXCEPTION 'Verification evidence mismatch' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_verified_customer_account BEFORE INSERT OR UPDATE OR DELETE ON verified_customer_accounts
    FOR EACH ROW EXECUTE FUNCTION guard_verified_customer_account();
