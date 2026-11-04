-- Private customer-session custody stores only an opaque verifier. Bearer
-- possession and the final clock check belong to the trusted Rust caller.
CREATE TABLE customer_sessions (
    verifier bytea PRIMARY KEY CHECK (octet_length(verifier)=32),
    account_id bigint NOT NULL REFERENCES verified_customer_accounts(id),
    credential_revision integer NOT NULL CHECK (credential_revision>0),
    created_at bigint NOT NULL CHECK (created_at>=0),
    expires_at bigint NOT NULL CHECK (expires_at>created_at),
    rotated_from bytea UNIQUE REFERENCES customer_sessions(verifier),
    revoked_at bigint CHECK (revoked_at>=created_at),
    revocation_kind text CHECK (revocation_kind IN ('logout','rotation')),
    revoked_xid xid8,
    created_xid xid8 NOT NULL,
    CHECK ((revoked_at IS NULL AND revocation_kind IS NULL AND revoked_xid IS NULL)
        OR (revoked_at IS NOT NULL AND revocation_kind IS NOT NULL AND revoked_xid IS NOT NULL))
);
CREATE INDEX customer_sessions_rotation_transaction ON customer_sessions(revoked_xid)
    WHERE revocation_kind='rotation';

CREATE FUNCTION guard_customer_session() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    current_revision integer;
    credential_changed_at bigint;
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Customer sessions cannot be deleted' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Customer session custody requires read committed' USING ERRCODE='23514';
    END IF;

    -- Callers claim the account before the session row. This serializes session
    -- issuance and rotation with credential replacement, including lock waits.
    PERFORM 1 FROM verified_customer_accounts
        WHERE id=NEW.account_id AND verified_at<=NEW.created_at FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Customer session account unavailable' USING ERRCODE='23514';
    END IF;

    IF TG_OP='INSERT' THEN
        IF NEW.created_xid IS NOT NULL OR NEW.revoked_at IS NOT NULL
            OR NEW.revocation_kind IS NOT NULL OR NEW.revoked_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid customer session insertion' USING ERRCODE='23514';
        END IF;
        IF NEW.rotated_from IS NULL THEN
            IF NEW.expires_at<=NEW.created_at OR NEW.expires_at-NEW.created_at<>43200 THEN
                RAISE EXCEPTION 'Invalid customer session lifetime' USING ERRCODE='23514';
            END IF;
        ELSIF NOT EXISTS (
            SELECT 1 FROM customer_sessions AS predecessor
            WHERE predecessor.verifier=NEW.rotated_from
                AND predecessor.account_id=NEW.account_id
                AND predecessor.credential_revision=NEW.credential_revision
                AND predecessor.expires_at=NEW.expires_at
                AND predecessor.revoked_at=NEW.created_at
                AND predecessor.revocation_kind='rotation'
                AND predecessor.revoked_xid=pg_current_xact_id()
                AND predecessor.created_xid<>pg_current_xact_id()
                AND NEW.created_at>=predecessor.created_at
                AND NEW.created_at<predecessor.expires_at
        ) THEN
            RAISE EXCEPTION 'Customer session predecessor mismatch' USING ERRCODE='23514';
        END IF;
    ELSE
        IF (NEW.verifier,NEW.account_id,NEW.credential_revision,NEW.created_at,
            NEW.expires_at,NEW.rotated_from,NEW.created_xid)
            IS DISTINCT FROM (OLD.verifier,OLD.account_id,OLD.credential_revision,OLD.created_at,
                OLD.expires_at,OLD.rotated_from,OLD.created_xid)
            OR OLD.revoked_at IS NOT NULL OR OLD.revocation_kind IS NOT NULL
            OR OLD.revoked_xid IS NOT NULL OR NEW.revoked_at IS NULL
            OR NEW.revocation_kind IS NULL OR NEW.revoked_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid customer session transition' USING ERRCODE='23514';
        END IF;
        IF NEW.revocation_kind='rotation' THEN
            IF OLD.created_xid=pg_current_xact_id()
                OR NEW.revoked_at<OLD.created_at OR NEW.revoked_at>=OLD.expires_at
                OR EXISTS (SELECT 1 FROM customer_sessions
                    WHERE revoked_xid=pg_current_xact_id() AND revocation_kind='rotation') THEN
                RAISE EXCEPTION 'Invalid customer session rotation' USING ERRCODE='23514';
            END IF;
        ELSIF NEW.revocation_kind<>'logout' OR NEW.revoked_at<OLD.created_at THEN
            RAISE EXCEPTION 'Invalid customer session revocation' USING ERRCODE='23514';
        END IF;
    END IF;

    -- Logout remains possible after a password change; issuance and rotation
    -- must instead be bound to the current credential under the account lock.
    IF TG_OP='INSERT' OR NEW.revocation_kind='rotation' THEN
        SELECT revision,changed_at INTO current_revision,credential_changed_at
            FROM customer_password_credentials WHERE account_id=NEW.account_id;
        IF current_revision IS NULL OR NEW.credential_revision<>current_revision
            OR NEW.created_at<credential_changed_at THEN
            RAISE EXCEPTION 'Customer session credential mismatch' USING ERRCODE='23514';
        END IF;
    END IF;
    IF TG_OP='INSERT' THEN
        NEW.created_xid := pg_current_xact_id();
    ELSE
        NEW.revoked_xid := pg_current_xact_id();
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_customer_session
    BEFORE INSERT OR UPDATE OR DELETE ON customer_sessions
    FOR EACH ROW EXECUTE FUNCTION guard_customer_session();

-- A rotation cannot commit or be flushed without its exact immutable successor.
CREATE FUNCTION require_customer_session_rotation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.revocation_kind='rotation' AND NOT EXISTS (
        SELECT 1 FROM customer_sessions AS successor
        WHERE successor.rotated_from=NEW.verifier
            AND successor.account_id=NEW.account_id
            AND successor.credential_revision=NEW.credential_revision
            AND successor.created_at=NEW.revoked_at
            AND successor.expires_at=NEW.expires_at
            AND successor.created_xid=NEW.revoked_xid
    ) THEN
        RAISE EXCEPTION 'Customer session rotation has no successor' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_customer_session_rotation
    AFTER UPDATE ON customer_sessions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION require_customer_session_rotation();
