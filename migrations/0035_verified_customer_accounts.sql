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
        AND consumed_at=NEW.verified_at) THEN
        RAISE EXCEPTION 'Verification evidence mismatch' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_verified_customer_account BEFORE INSERT OR UPDATE OR DELETE ON verified_customer_accounts
    FOR EACH ROW EXECUTE FUNCTION guard_verified_customer_account();
