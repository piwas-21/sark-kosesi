-- Private one-use custody only. No account, outbox, delivery or authority is seeded.
CREATE TABLE email_verification_challenges (
    verifier bytea PRIMARY KEY CHECK (octet_length(verifier)=32),
    purpose text NOT NULL CHECK (purpose='email-verification-v1'),
    email_identity text NOT NULL CHECK (octet_length(email_identity) BETWEEN 3 AND 254),
    created_at bigint NOT NULL CHECK (created_at>=0),
    expires_at bigint NOT NULL CHECK (expires_at>created_at AND expires_at-created_at=900),
    consumed_at bigint CHECK (consumed_at>=created_at AND consumed_at<expires_at)
);
CREATE INDEX email_verification_expiry ON email_verification_challenges(expires_at);
CREATE FUNCTION guard_email_verification_challenge() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.verifier,NEW.purpose,NEW.email_identity,NEW.created_at,NEW.expires_at)
        IS DISTINCT FROM (OLD.verifier,OLD.purpose,OLD.email_identity,OLD.created_at,OLD.expires_at)
        OR OLD.consumed_at IS NOT NULL OR NEW.consumed_at IS NULL THEN
        RAISE EXCEPTION 'Invalid verification challenge transition' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_email_verification_challenge BEFORE UPDATE ON email_verification_challenges
    FOR EACH ROW EXECUTE FUNCTION guard_email_verification_challenge();
