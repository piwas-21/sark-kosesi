-- Optional private synchronizer custody must be created with its exact session.
CREATE TABLE customer_session_csrf (
    session_verifier bytea PRIMARY KEY REFERENCES customer_sessions(verifier),
    csrf_digest bytea NOT NULL CHECK (octet_length(csrf_digest)=32),
    created_xid xid8 NOT NULL
);

CREATE FUNCTION guard_customer_session_csrf() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        RAISE EXCEPTION 'Customer session CSRF binding is immutable' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation') <> 'read committed' OR NEW.created_xid IS NOT NULL THEN
        RAISE EXCEPTION 'Invalid customer session CSRF transaction' USING ERRCODE='23514';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM customer_sessions s WHERE s.verifier=NEW.session_verifier
        AND s.created_xid=pg_current_xact_id() AND s.rotated_from IS NULL
        AND s.revoked_at IS NULL AND s.revocation_kind IS NULL AND s.revoked_xid IS NULL) THEN
        RAISE EXCEPTION 'Customer session CSRF requires its new session' USING ERRCODE='23514';
    END IF;
    NEW.created_xid := pg_current_xact_id();
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_customer_session_csrf BEFORE INSERT OR UPDATE OR DELETE ON customer_session_csrf
    FOR EACH ROW EXECUTE FUNCTION guard_customer_session_csrf();
