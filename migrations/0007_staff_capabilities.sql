-- Empty on installation. P4 owns actual staff login, MFA verification and issuance.
CREATE TABLE staff_accounts (
    id text PRIMARY KEY CHECK (id ~ '^[a-zA-Z0-9-]{1,128}$'),
    enabled boolean NOT NULL DEFAULT false
);
CREATE TABLE staff_capabilities (
    staff_id text NOT NULL REFERENCES staff_accounts(id),
    capability text NOT NULL CHECK (capability IN ('catalogue.manage','warehouse.manage')),
    PRIMARY KEY (staff_id,capability)
);
CREATE TABLE staff_sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash)=32),
    staff_id text NOT NULL REFERENCES staff_accounts(id),
    created_at bigint NOT NULL CHECK (created_at>=0),
    expires_at bigint NOT NULL CONSTRAINT staff_session_window CHECK (expires_at>created_at AND expires_at-created_at<=43200),
    mfa_verified_at bigint CHECK (mfa_verified_at>=created_at AND mfa_verified_at<expires_at),
    revoked_at bigint CHECK (revoked_at>=created_at)
);
CREATE INDEX staff_sessions_owner ON staff_sessions(staff_id);
CREATE FUNCTION guard_staff_session_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.token_hash,NEW.staff_id,NEW.created_at) IS DISTINCT FROM (OLD.token_hash,OLD.staff_id,OLD.created_at) THEN
        RAISE EXCEPTION 'Session identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_staff_session_identity BEFORE UPDATE ON staff_sessions
FOR EACH ROW EXECUTE FUNCTION guard_staff_session_identity();
