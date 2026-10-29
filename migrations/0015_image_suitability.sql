-- Exact private image suitability decisions. No public derivative or approval is seeded.
CREATE TABLE catalogue_image_reviews (
    sku text NOT NULL,
    asset_id text NOT NULL,
    recipe text NOT NULL DEFAULT 'image-v1' CHECK(recipe='image-v1'),
    bom_revision integer NOT NULL,
    specification_revision integer NOT NULL,
    rights_revision integer NOT NULL,
    revision integer NOT NULL CHECK(revision>0),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    decision text NOT NULL CHECK(decision IN ('approved','rejected')),
    evidence_ref text NOT NULL CHECK(length(btrim(evidence_ref)) BETWEEN 1 AND 500),
    qualification_ref text NOT NULL CHECK(length(btrim(qualification_ref)) BETWEEN 1 AND 500),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    evaluated_at bigint NOT NULL CHECK(evaluated_at>=0),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(sku,asset_id,bom_revision,specification_revision,rights_revision,revision),
    FOREIGN KEY(sku,bom_revision) REFERENCES catalogue_boms(sku,revision),
    FOREIGN KEY(sku,specification_revision) REFERENCES catalogue_specification_revisions(sku,revision),
    FOREIGN KEY(asset_id,rights_revision) REFERENCES catalogue_media_rights(asset_id,revision),
    FOREIGN KEY(asset_id,recipe) REFERENCES catalogue_image_originals(asset_id,recipe)
);
CREATE FUNCTION guard_image_review() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_review integer; specification catalogue_specification_revisions%ROWTYPE;
    metadata catalogue_media_assets%ROWTYPE; original catalogue_image_originals%ROWTYPE;
    rights catalogue_media_rights%ROWTYPE; bom_actor text; identity_actor text;
    current_bom integer; current_specification integer; current_rights integer;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Image review history is immutable' USING ERRCODE='23514';
    END IF;
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    UPDATE catalogue_media_serialization SET anchor=anchor WHERE asset_id=NEW.asset_id;
    IF NOT FOUND THEN RAISE EXCEPTION 'Missing image serialization' USING ERRCODE='23514'; END IF;
    SELECT * INTO specification FROM catalogue_specification_revisions WHERE sku=NEW.sku AND revision=NEW.specification_revision;
    SELECT * INTO metadata FROM catalogue_media_assets WHERE asset_id=NEW.asset_id;
    SELECT * INTO original FROM catalogue_image_originals WHERE asset_id=NEW.asset_id AND recipe=NEW.recipe;
    SELECT * INTO rights FROM catalogue_media_rights WHERE asset_id=NEW.asset_id AND revision=NEW.rights_revision;
    SELECT e.actor INTO bom_actor FROM catalogue_boms b JOIN catalogue_definition_events e ON e.effect_key=b.creation_effect_key
        WHERE b.sku=NEW.sku AND b.revision=NEW.bom_revision AND b.sealed AND definition_event_matches(e);
    SELECT e.actor INTO identity_actor FROM catalogue_items i JOIN catalogue_definition_events e ON e.effect_key=i.creation_effect_key
        WHERE i.sku=NEW.sku AND NOT i.fixture AND definition_event_matches(e);
    IF specification.bom_revision IS DISTINCT FROM NEW.bom_revision OR original.asset_id IS NULL OR rights.asset_id IS NULL
        OR bom_actor IS NULL OR identity_actor IS NULL
        OR NEW.actor IN (specification.actor,metadata.actor,original.actor,rights.actor,bom_actor,identity_actor) THEN
        RAISE EXCEPTION 'Image review requires merchant sources and an independent actor' USING ERRCODE='23514';
    END IF;
    IF NEW.decision='approved' THEN
        SELECT max(revision) INTO current_bom FROM catalogue_boms WHERE sku=NEW.sku AND sealed;
        SELECT max(revision) INTO current_specification FROM catalogue_specification_revisions WHERE sku=NEW.sku;
        SELECT max(revision) INTO current_rights FROM catalogue_media_rights WHERE asset_id=NEW.asset_id;
        IF (NEW.bom_revision,NEW.specification_revision,NEW.rights_revision) IS DISTINCT FROM (current_bom,current_specification,current_rights)
            OR metadata.payload->>'kind' IS DISTINCT FROM 'original-photo' OR metadata.payload->'fixture' IS DISTINCT FROM 'false'::jsonb
            OR rights.change->>'action' IS DISTINCT FROM 'grant' OR rights.change->>'use_scope' IS DISTINCT FROM 'product-photography'
            OR (rights.change->>'valid_from')::bigint IS NULL OR (rights.change->>'valid_from')::bigint>NEW.evaluated_at
            OR rights.change->'term'->>'kind' IS NULL OR rights.change->'term'->>'kind' NOT IN ('perpetual','until')
            OR (rights.change->'term'->>'kind'='until' AND ((rights.change->'term'->>'expires_at')::bigint IS NULL OR (rights.change->'term'->>'expires_at')::bigint<=NEW.evaluated_at)) THEN
            RAISE EXCEPTION 'Image approval requires current sources and active product rights' USING ERRCODE='23514';
        END IF;
    END IF;
    SELECT coalesce(max(revision),0) INTO current_review FROM catalogue_image_reviews
        WHERE sku=NEW.sku AND asset_id=NEW.asset_id AND bom_revision=NEW.bom_revision
            AND specification_revision=NEW.specification_revision AND rights_revision=NEW.rights_revision;
    IF current_review=2147483647 OR NEW.revision<>current_review+1 THEN
        RAISE EXCEPTION 'Image review revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_image_review BEFORE INSERT OR UPDATE OR DELETE ON catalogue_image_reviews FOR EACH ROW EXECUTE FUNCTION guard_image_review();
