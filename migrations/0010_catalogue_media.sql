-- No assets, licences, uploads or public content are seeded.
CREATE TABLE catalogue_media_assets (
    asset_id text PRIMARY KEY CHECK (asset_id ~ '^[a-zA-Z0-9-]{1,128}$'),
    effect_key text UNIQUE NOT NULL CHECK (effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload)='object' AND octet_length(payload::text)<=8192),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
-- One content digest has one permanent classification; a new alias cannot turn
-- a known fixture/generated image into an original merchant photograph.
CREATE UNIQUE INDEX catalogue_media_content_identity ON catalogue_media_assets((payload->'sha256'));
CREATE TABLE catalogue_media_rights (
    asset_id text NOT NULL REFERENCES catalogue_media_assets(asset_id),
    revision integer NOT NULL CHECK (revision>0),
    effect_key text UNIQUE NOT NULL CHECK (effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    change jsonb NOT NULL CHECK (jsonb_typeof(change)='object' AND octet_length(change::text)<=8192),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (asset_id,revision)
);
-- A separate mutable serialization row preserves immutable asset metadata while
-- forcing stale repeatable-read raw writers to retry after any rights change.
CREATE TABLE catalogue_media_serialization (
    asset_id text PRIMARY KEY REFERENCES catalogue_media_assets(asset_id),
    anchor boolean NOT NULL DEFAULT true CHECK (anchor)
);
ALTER TABLE catalogue_media_assets ADD CONSTRAINT media_serialization_required
FOREIGN KEY(asset_id) REFERENCES catalogue_media_serialization(asset_id) DEFERRABLE INITIALLY DEFERRED;
CREATE FUNCTION initialize_media_serialization() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO catalogue_media_serialization(asset_id) VALUES(NEW.asset_id);
    RETURN NEW;
END;
$$;
CREATE TRIGGER initialize_media_serialization AFTER INSERT ON catalogue_media_assets
FOR EACH ROW EXECUTE FUNCTION initialize_media_serialization();
CREATE FUNCTION guard_media_asset_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Media metadata is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER guard_media_asset_history BEFORE UPDATE OR DELETE ON catalogue_media_assets
FOR EACH ROW EXECUTE FUNCTION guard_media_asset_history();
CREATE FUNCTION guard_media_rights_history() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer; metadata jsonb;
BEGIN
    IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Media rights history is immutable' USING ERRCODE='23514'; END IF;
    UPDATE catalogue_media_serialization SET anchor=anchor WHERE asset_id=NEW.asset_id;
    IF NOT FOUND THEN RAISE EXCEPTION 'Missing media identity' USING ERRCODE='23514'; END IF;
    SELECT payload INTO metadata FROM catalogue_media_assets WHERE asset_id=NEW.asset_id;
    IF NEW.change->>'action'='grant' AND NEW.change->>'use_scope'='product-photography' AND
        (metadata->>'kind' IS DISTINCT FROM 'original-photo' OR metadata->'fixture' IS DISTINCT FROM 'false'::jsonb) THEN
        RAISE EXCEPTION 'Fixture/generated media cannot receive product rights' USING ERRCODE='23514';
    END IF;
    SELECT coalesce(max(revision),0) INTO current_revision FROM catalogue_media_rights WHERE asset_id=NEW.asset_id;
    IF current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
        RAISE EXCEPTION 'Media rights revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_media_rights_history BEFORE INSERT OR UPDATE OR DELETE ON catalogue_media_rights
FOR EACH ROW EXECUTE FUNCTION guard_media_rights_history();
