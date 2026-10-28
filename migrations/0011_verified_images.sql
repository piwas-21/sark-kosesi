-- Private Piwas PostgreSQL storage adapter; no public file or delivery route.
CREATE TABLE catalogue_image_originals (
    asset_id text NOT NULL REFERENCES catalogue_media_assets(asset_id),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    sha256 bytea NOT NULL CHECK(octet_length(sha256)=32),
    bytes bytea NOT NULL CHECK(octet_length(bytes) BETWEEN 1 AND 20971520),
    image_type text NOT NULL CHECK(image_type IN ('jpeg','png','webp')),
    source_width_px integer NOT NULL CHECK(source_width_px BETWEEN 1 AND 16384),
    source_height_px integer NOT NULL CHECK(source_height_px BETWEEN 1 AND 16384),
    display_width_px integer NOT NULL CHECK(display_width_px BETWEEN 1 AND 16384),
    display_height_px integer NOT NULL CHECK(display_height_px BETWEEN 1 AND 16384),
    recipe text NOT NULL DEFAULT 'image-v1' CHECK(recipe='image-v1'),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(asset_id,recipe),
    CHECK(sha256=sha256(bytes)),
    CHECK(source_width_px::bigint*source_height_px<=67108864),
    CHECK((source_width_px=display_width_px AND source_height_px=display_height_px) OR (source_width_px=display_height_px AND source_height_px=display_width_px))
);
CREATE TABLE catalogue_image_renditions (
    asset_id text NOT NULL,
    recipe text NOT NULL DEFAULT 'image-v1' CHECK(recipe='image-v1'),
    edge integer NOT NULL CHECK(edge IN (320,640,960)),
    width_px integer NOT NULL CHECK(width_px BETWEEN 1 AND edge),
    height_px integer NOT NULL CHECK(height_px BETWEEN 1 AND edge),
    image_type text NOT NULL CHECK(image_type IN ('jpeg','png')),
    sha256 bytea NOT NULL CHECK(octet_length(sha256)=32),
    bytes bytea NOT NULL CHECK(octet_length(bytes) BETWEEN 1 AND 256000),
    PRIMARY KEY(asset_id,recipe,edge),
    FOREIGN KEY(asset_id,recipe) REFERENCES catalogue_image_originals(asset_id,recipe),
    CHECK(sha256=sha256(bytes))
);
CREATE FUNCTION guard_image_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Verified image history is immutable' USING ERRCODE='23514'; END;
$$;
CREATE TRIGGER guard_image_originals BEFORE UPDATE OR DELETE ON catalogue_image_originals FOR EACH ROW EXECUTE FUNCTION guard_image_history();
CREATE TRIGGER guard_image_renditions BEFORE UPDATE OR DELETE ON catalogue_image_renditions FOR EACH ROW EXECUTE FUNCTION guard_image_history();
CREATE FUNCTION bind_image_metadata() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE metadata jsonb;
BEGIN
    SELECT payload INTO metadata FROM catalogue_media_assets WHERE asset_id=NEW.asset_id;
    IF metadata->'sha256' IS DISTINCT FROM to_jsonb(ARRAY(SELECT get_byte(NEW.sha256,n) FROM generate_series(0,31)n)) OR
        metadata->>'image_type' IS DISTINCT FROM NEW.image_type OR metadata->'width_px' IS DISTINCT FROM to_jsonb(NEW.source_width_px) OR
        metadata->'height_px' IS DISTINCT FROM to_jsonb(NEW.source_height_px) THEN
        RAISE EXCEPTION 'Image bytes do not match immutable metadata' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER bind_image_metadata BEFORE INSERT ON catalogue_image_originals FOR EACH ROW EXECUTE FUNCTION bind_image_metadata();
CREATE FUNCTION complete_image_set() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE identity text; longest integer; edges integer[]; expected integer[];
BEGIN
    identity=NEW.asset_id;
    SELECT greatest(display_width_px,display_height_px) INTO longest FROM catalogue_image_originals WHERE asset_id=identity AND recipe=NEW.recipe;
    SELECT array_agg(edge ORDER BY edge) INTO edges FROM catalogue_image_renditions WHERE asset_id=identity AND recipe=NEW.recipe;
    expected=ARRAY[320];
    IF longest>320 THEN expected=array_append(expected,640); END IF;
    IF longest>640 THEN expected=array_append(expected,960); END IF;
    IF edges IS DISTINCT FROM expected OR EXISTS(SELECT 1 FROM catalogue_image_renditions WHERE asset_id=identity AND recipe=NEW.recipe AND greatest(width_px,height_px)<>least(edge,longest)) THEN
        RAISE EXCEPTION 'Incomplete image rendition set' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER complete_image_original AFTER INSERT ON catalogue_image_originals DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION complete_image_set();
CREATE CONSTRAINT TRIGGER complete_image_rendition AFTER INSERT ON catalogue_image_renditions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION complete_image_set();
