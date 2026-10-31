-- Empty credentials/baskets on installation. The quota row grants no user authority.
CREATE FUNCTION guest_lines_valid(payload jsonb) RETURNS boolean LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE item jsonb; sku text; seen text[]:=ARRAY[]::text[];
BEGIN
    IF jsonb_typeof(payload) IS DISTINCT FROM 'array' THEN RETURN false; END IF;
    IF jsonb_array_length(payload)>20 THEN RETURN false; END IF;
    FOR item IN SELECT value FROM jsonb_array_elements(payload) LOOP
        IF jsonb_typeof(item) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
        IF (SELECT count(*) FROM jsonb_object_keys(item))<>2 OR NOT item ?& ARRAY['sku','quantity']
            OR jsonb_typeof(item->'sku') IS DISTINCT FROM 'string'
            OR jsonb_typeof(item->'quantity') IS DISTINCT FROM 'number' THEN RETURN false; END IF;
        sku:=item->>'sku';
        IF length(sku) NOT BETWEEN 1 AND 65 OR sku !~ '^[A-Z0-9][A-Z0-9-]*$'
            OR sku !~ '[A-Z0-9]$' OR sku='DEMO' OR sku LIKE 'DEMO-%' OR sku=ANY(seen)
            OR (item->>'quantity') !~ '^[1-9][0-9]{0,2}$' THEN RETURN false; END IF;
        IF (item->>'quantity')::integer>100 THEN RETURN false; END IF;
        seen:=array_append(seen,sku);
    END LOOP;
    RETURN true;
END;
$$;
CREATE TABLE guest_baskets (
    id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
    created_at bigint NOT NULL CHECK(created_at>=0),
    revision integer NOT NULL DEFAULT 0 CHECK(revision>=0),
    lines jsonb NOT NULL DEFAULT '[]' CHECK(octet_length(lines::text)<=8192 AND guest_lines_valid(lines)),
    last_effect_key text,
    rate_window bigint NOT NULL DEFAULT 0 CHECK(rate_window>=0),
    mutation_count integer NOT NULL DEFAULT 0 CHECK(mutation_count BETWEEN 0 AND 1000),
    CHECK((revision=0 AND last_effect_key IS NULL AND lines='[]'::jsonb) OR (revision>0 AND last_effect_key IS NOT NULL))
);
CREATE TABLE guest_sessions (
    token_hash bytea PRIMARY KEY CHECK(octet_length(token_hash)=32),
    basket_id text NOT NULL REFERENCES guest_baskets(id),
    csrf_token text NOT NULL CHECK(csrf_token ~ '^[a-f0-9]{64}$'),
    created_at bigint NOT NULL CHECK(created_at>=0),
    expires_at bigint NOT NULL CHECK(expires_at>created_at AND expires_at-created_at<=604800)
);
CREATE INDEX guest_sessions_basket ON guest_sessions(basket_id);
CREATE INDEX guest_sessions_expiry ON guest_sessions(expires_at);
CREATE TABLE guest_session_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash bytea NOT NULL CHECK(octet_length(token_hash)=32),
    basket_id text NOT NULL REFERENCES guest_baskets(id),
    action text NOT NULL CHECK(action IN ('issued','revoked')),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE FUNCTION guest_session_audit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Guest credential identity is immutable' USING ERRCODE='23514'; END IF;
    IF TG_OP='INSERT' THEN
        INSERT INTO guest_session_events(token_hash,basket_id,action) VALUES(NEW.token_hash,NEW.basket_id,'issued');
        RETURN NEW;
    END IF;
    INSERT INTO guest_session_events(token_hash,basket_id,action) VALUES(OLD.token_hash,OLD.basket_id,'revoked');
    RETURN OLD;
END;
$$;
CREATE TRIGGER guest_session_audit AFTER INSERT OR UPDATE OR DELETE ON guest_sessions FOR EACH ROW EXECUTE FUNCTION guest_session_audit();
CREATE TABLE guest_issue_quota (
    singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
    rate_window bigint NOT NULL DEFAULT 0 CHECK(rate_window>=0),
    issued integer NOT NULL DEFAULT 0 CHECK(issued BETWEEN 0 AND 1000)
);
INSERT INTO guest_issue_quota(singleton) VALUES(true);
CREATE TABLE guest_basket_effects (
    basket_id text NOT NULL REFERENCES guest_baskets(id),
    effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    expected_revision integer NOT NULL CHECK(expected_revision BETWEEN 0 AND 2147483646),
    revision integer NOT NULL CHECK(revision=expected_revision+1),
    lines jsonb NOT NULL CHECK(octet_length(lines::text)<=8192 AND guest_lines_valid(lines)),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(basket_id,effect_key),
    UNIQUE(basket_id,revision)
);
ALTER TABLE guest_baskets ADD FOREIGN KEY(id,last_effect_key) REFERENCES guest_basket_effects(basket_id,effect_key) DEFERRABLE INITIALLY DEFERRED;
CREATE FUNCTION guard_guest_basket() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.id,NEW.created_at) IS DISTINCT FROM (OLD.id,OLD.created_at) THEN
        RAISE EXCEPTION 'Guest basket identity is immutable' USING ERRCODE='23514';
    END IF;
    IF (NEW.revision,NEW.lines,NEW.last_effect_key) IS DISTINCT FROM (OLD.revision,OLD.lines,OLD.last_effect_key) THEN
        IF OLD.revision=2147483647 OR NEW.revision<>OLD.revision+1
            OR NOT EXISTS(SELECT 1 FROM guest_basket_effects e WHERE e.basket_id=NEW.id AND e.effect_key=NEW.last_effect_key AND e.revision=NEW.revision AND e.lines=NEW.lines) THEN
            RAISE EXCEPTION 'Guest basket requires next exact receipt' USING ERRCODE='23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_guest_basket BEFORE UPDATE ON guest_baskets FOR EACH ROW EXECUTE FUNCTION guard_guest_basket();
CREATE FUNCTION guard_guest_receipt() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer;
BEGIN
    IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Guest effect history is immutable' USING ERRCODE='23514'; END IF;
    UPDATE guest_baskets SET revision=revision WHERE id=NEW.basket_id RETURNING revision INTO current_revision;
    IF current_revision IS DISTINCT FROM NEW.expected_revision THEN
        RAISE EXCEPTION 'Guest effect must follow current basket' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_guest_receipt BEFORE INSERT OR UPDATE OR DELETE ON guest_basket_effects FOR EACH ROW EXECUTE FUNCTION guard_guest_receipt();
CREATE FUNCTION guest_receipt_applied() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM guest_baskets WHERE id=NEW.basket_id AND revision>=NEW.revision) THEN
        RAISE EXCEPTION 'Unapplied guest receipt cannot commit' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER guest_receipt_applied AFTER INSERT ON guest_basket_effects DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION guest_receipt_applied();
CREATE FUNCTION guard_guest_session_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Guest session audit is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER guard_guest_session_event BEFORE UPDATE OR DELETE ON guest_session_events FOR EACH ROW EXECUTE FUNCTION guard_guest_session_event();
