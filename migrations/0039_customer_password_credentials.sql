-- Private password proof custody. A challenge is scoped by this table and by
-- the versioned Rust verifier domain; neither table stores bearer tokens.
CREATE TABLE customer_password_challenges (
    verifier bytea PRIMARY KEY CHECK (octet_length(verifier)=32),
    account_id bigint NOT NULL REFERENCES verified_customer_accounts(id),
    expected_revision integer NOT NULL CHECK (expected_revision>=0),
    created_at bigint NOT NULL CHECK (created_at>=0),
    expires_at bigint NOT NULL CHECK (expires_at>created_at AND expires_at-created_at=900),
    consumed_at bigint CHECK (consumed_at>=created_at AND consumed_at<expires_at),
    consumed_xid xid8
);

CREATE TABLE customer_password_credentials (
    account_id bigint PRIMARY KEY REFERENCES verified_customer_accounts(id),
    revision integer NOT NULL CHECK (revision>0),
    -- Canonical unpadded base64: the final character also constrains unused bits.
    password_hash text NOT NULL CHECK (password_hash ~
        '^\$argon2id\$v=19\$m=19456,t=2,p=1\$[A-Za-z0-9+/]{21}[AQgw]\$[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]$'),
    changed_at bigint NOT NULL CHECK (changed_at>=0),
    challenge_verifier bytea NOT NULL UNIQUE REFERENCES customer_password_challenges(verifier),
    changed_xid xid8 NOT NULL
);

CREATE FUNCTION guard_customer_password_challenge() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    current_revision integer;
    current_changed_at bigint;
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Password challenges are immutable' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Password custody requires read committed' USING ERRCODE='23514';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.consumed_at IS NOT NULL OR NEW.consumed_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid password challenge insertion' USING ERRCODE='23514';
        END IF;
    ELSIF (NEW.verifier,NEW.account_id,NEW.expected_revision,NEW.created_at,NEW.expires_at)
        IS DISTINCT FROM (OLD.verifier,OLD.account_id,OLD.expected_revision,OLD.created_at,OLD.expires_at)
        OR OLD.consumed_at IS NOT NULL OR OLD.consumed_xid IS NOT NULL
        OR NEW.consumed_at IS NULL OR NEW.consumed_xid IS NOT NULL THEN
        RAISE EXCEPTION 'Invalid password challenge transition' USING ERRCODE='23514';
    END IF;

    -- One lock orders issuance, consumption and credential replacement for an account.
    PERFORM 1 FROM verified_customer_accounts
        WHERE id=NEW.account_id AND verified_at<=NEW.created_at FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Password account unavailable' USING ERRCODE='23514';
    END IF;
    SELECT revision,changed_at INTO current_revision,current_changed_at
        FROM customer_password_credentials WHERE account_id=NEW.account_id;
    IF NEW.expected_revision<>COALESCE(current_revision,0)
        OR (current_changed_at IS NOT NULL AND NEW.created_at<current_changed_at) THEN
        RAISE EXCEPTION 'Password challenge revision mismatch' USING ERRCODE='23514';
    END IF;
    IF TG_OP='UPDATE' THEN
        NEW.consumed_xid := pg_current_xact_id();
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_customer_password_challenge
    BEFORE INSERT OR UPDATE OR DELETE ON customer_password_challenges
    FOR EACH ROW EXECUTE FUNCTION guard_customer_password_challenge();

CREATE FUNCTION guard_customer_password_credential() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    previous_revision integer;
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Password credentials cannot be deleted' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Password custody requires read committed' USING ERRCODE='23514';
    END IF;
    PERFORM 1 FROM verified_customer_accounts WHERE id=NEW.account_id FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Password account unavailable' USING ERRCODE='23514';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.changed_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid password credential insertion' USING ERRCODE='23514';
        END IF;
        previous_revision := 0;
    ELSE
        IF NEW.account_id IS DISTINCT FROM OLD.account_id
            OR NEW.changed_xid IS DISTINCT FROM OLD.changed_xid
            OR OLD.changed_xid=pg_current_xact_id()
            OR NEW.changed_at<OLD.changed_at THEN
            RAISE EXCEPTION 'Invalid password credential transition' USING ERRCODE='23514';
        END IF;
        previous_revision := OLD.revision;
    END IF;
    IF NEW.revision::bigint<>previous_revision::bigint+1 OR NOT EXISTS (
        SELECT 1 FROM customer_password_challenges
        WHERE verifier=NEW.challenge_verifier AND account_id=NEW.account_id
            AND expected_revision=previous_revision AND consumed_at=NEW.changed_at
            AND consumed_xid=pg_current_xact_id()
    ) THEN
        RAISE EXCEPTION 'Password change evidence mismatch' USING ERRCODE='23514';
    END IF;
    NEW.changed_xid := pg_current_xact_id();
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_customer_password_credential
    BEFORE INSERT OR UPDATE OR DELETE ON customer_password_credentials
    FOR EACH ROW EXECUTE FUNCTION guard_customer_password_credential();

-- A consumed proof cannot commit without its exact replacement in this same
-- transaction. Callers force this constraint before their final clock check.
CREATE FUNCTION require_customer_password_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.consumed_at IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM customer_password_credentials
        WHERE account_id=NEW.account_id AND challenge_verifier=NEW.verifier
            AND revision::bigint=NEW.expected_revision::bigint+1
            AND changed_at=NEW.consumed_at AND changed_xid=NEW.consumed_xid
    ) THEN
        RAISE EXCEPTION 'Password challenge has no credential change' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_customer_password_change
    AFTER UPDATE ON customer_password_challenges DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION require_customer_password_change();
