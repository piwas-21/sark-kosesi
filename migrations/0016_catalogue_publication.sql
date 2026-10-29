-- Explicit approved snapshots and stable family paths. No publication is seeded.
CREATE TABLE catalogue_publication_revisions (
    sku text NOT NULL REFERENCES catalogue_items(sku),
    locale text NOT NULL CHECK(locale IN ('nl','de','fr','en','ar','tr','kmr')),
    revision integer NOT NULL CHECK(revision>0),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    choice jsonb NOT NULL CHECK(jsonb_typeof(choice)='object' AND octet_length(choice::text)<=8192),
    evidence_ref text NOT NULL CHECK(length(btrim(evidence_ref)) BETWEEN 1 AND 500),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    evaluated_at bigint NOT NULL CHECK(evaluated_at>=0),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(sku,locale,revision)
);
CREATE TABLE catalogue_family_routes (
    family_code text NOT NULL REFERENCES product_families(code),
    locale text NOT NULL CHECK(locale IN ('nl','de','fr','en','ar','tr','kmr')),
    slug text NOT NULL CHECK(length(slug) BETWEEN 1 AND 100 AND slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    creation_effect_key text NOT NULL REFERENCES catalogue_publication_revisions(effect_key) DEFERRABLE INITIALLY DEFERRED,
    PRIMARY KEY(family_code,locale), UNIQUE(locale,slug)
);
CREATE FUNCTION publication_route_matches(candidate catalogue_family_routes) RETURNS boolean LANGUAGE SQL AS $$
    SELECT EXISTS(SELECT 1 FROM catalogue_publication_revisions p JOIN catalogue_variants v USING(sku)
        WHERE p.effect_key=(candidate).creation_effect_key AND p.locale=(candidate).locale
            AND v.family_code=(candidate).family_code AND p.choice->>'action'='activate'
            AND p.choice->'selection'->>'family_slug'=(candidate).slug);
$$;
-- Version/decision integrity complements Rust's complete typed payload validation.
-- Never use this predicate alone to return unvalidated source data to the public.
CREATE FUNCTION publication_sources_current(item text,language text,choice jsonb,now_seconds bigint)
RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE selected jsonb; facts jsonb; image jsonb; copy catalogue_translation_revisions%ROWTYPE;
    source catalogue_translation_revisions%ROWTYPE; rights catalogue_media_rights%ROWTYPE;
    metadata catalogue_media_assets%ROWTYPE; bom integer; specification integer; price integer;
    fact_review integer; image_review integer; copy_review integer; source_review integer; permission integer;
BEGIN
    IF choice->>'action' IS DISTINCT FROM 'activate' OR now_seconds<0 THEN RETURN false; END IF;
    selected=choice->'selection'; facts=selected->'facts'; image=selected->'image';
    IF NOT EXISTS(SELECT 1 FROM catalogue_items WHERE sku=item AND NOT fixture) THEN RETURN false; END IF;
    SELECT max(revision) INTO bom FROM catalogue_boms WHERE sku=item AND sealed;
    SELECT max(revision) INTO specification FROM catalogue_specification_revisions WHERE sku=item;
    SELECT max(revision) INTO price FROM catalogue_price_revisions WHERE sku=item;
    IF ((facts->>'bom_revision')::integer,(facts->>'specification_revision')::integer,(facts->>'price_revision')::integer)
        IS DISTINCT FROM (bom,specification,price) OR bom IS NULL OR specification IS NULL OR price IS NULL
        OR ((image->>'bom_revision')::integer,(image->>'specification_revision')::integer) IS DISTINCT FROM (bom,specification)
        OR NOT EXISTS(SELECT 1 FROM catalogue_specification_revisions WHERE sku=item AND revision=specification AND bom_revision=bom)
        OR NOT EXISTS(SELECT 1 FROM catalogue_price_revisions WHERE sku=item AND revision=price AND currency='EUR' AND minor_units BETWEEN 0 AND 9007199254740991) THEN RETURN false; END IF;
    SELECT max(revision) INTO fact_review FROM catalogue_factual_reviews WHERE sku=item AND bom_revision=bom AND specification_revision=specification AND price_revision=price;
    IF fact_review IS DISTINCT FROM (selected->>'factual_review_revision')::integer OR fact_review IS NULL
        OR NOT EXISTS(SELECT 1 FROM catalogue_factual_reviews WHERE sku=item AND bom_revision=bom AND specification_revision=specification AND price_revision=price AND revision=fact_review AND decision='approved') THEN RETURN false; END IF;
    SELECT max(revision) INTO permission FROM catalogue_media_rights WHERE asset_id=image->>'asset_id';
    SELECT * INTO metadata FROM catalogue_media_assets WHERE asset_id=image->>'asset_id';
    SELECT * INTO rights FROM catalogue_media_rights WHERE asset_id=image->>'asset_id' AND revision=permission;
    IF permission IS DISTINCT FROM (image->>'rights_revision')::integer OR permission IS NULL
        OR metadata.payload->>'kind' IS DISTINCT FROM 'original-photo' OR metadata.payload->'fixture' IS DISTINCT FROM 'false'::jsonb
        OR NOT coalesce(rights.change->>'action'='grant' AND rights.change->>'use_scope'='product-photography'
            AND (rights.change->>'valid_from')::bigint BETWEEN 0 AND now_seconds
            AND (rights.change->'term'->>'kind'='perpetual' OR (rights.change->'term'->>'kind'='until' AND (rights.change->'term'->>'expires_at')::bigint>now_seconds)),false)
        OR NOT EXISTS(SELECT 1 FROM catalogue_image_originals WHERE asset_id=image->>'asset_id' AND recipe='image-v1') THEN RETURN false; END IF;
    SELECT max(revision) INTO image_review FROM catalogue_image_reviews WHERE sku=item AND asset_id=image->>'asset_id' AND bom_revision=bom AND specification_revision=specification AND rights_revision=permission;
    IF image_review IS DISTINCT FROM (selected->>'image_review_revision')::integer OR image_review IS NULL
        OR NOT EXISTS(SELECT 1 FROM catalogue_image_reviews WHERE sku=item AND asset_id=image->>'asset_id' AND bom_revision=bom AND specification_revision=specification AND rights_revision=permission AND revision=image_review AND decision='approved') THEN RETURN false; END IF;
    SELECT * INTO copy FROM catalogue_translation_revisions WHERE sku=item AND locale=language ORDER BY revision DESC LIMIT 1;
    IF copy.revision IS DISTINCT FROM (selected->>'translation_revision')::integer OR copy.revision IS NULL OR copy.specification_revision IS DISTINCT FROM specification THEN RETURN false; END IF;
    SELECT max(revision) INTO copy_review FROM catalogue_translation_reviews WHERE sku=item AND locale=language AND translation_revision=copy.revision;
    IF copy_review IS DISTINCT FROM (selected->>'translation_review_revision')::integer OR copy_review IS NULL
        OR NOT EXISTS(SELECT 1 FROM catalogue_translation_reviews WHERE sku=item AND locale=language AND translation_revision=copy.revision AND revision=copy_review AND decision='approved') THEN RETURN false; END IF;
    IF language='nl' THEN
        IF copy.source_revision IS NOT NULL OR selected->'source_review_revision' IS DISTINCT FROM 'null'::jsonb THEN RETURN false; END IF;
    ELSE
        SELECT * INTO source FROM catalogue_translation_revisions WHERE sku=item AND locale='nl' ORDER BY revision DESC LIMIT 1;
        IF source.revision IS NULL OR source.revision IS DISTINCT FROM copy.source_revision OR source.specification_revision IS DISTINCT FROM specification THEN RETURN false; END IF;
        SELECT max(revision) INTO source_review FROM catalogue_translation_reviews WHERE sku=item AND locale='nl' AND translation_revision=source.revision;
        IF source_review IS DISTINCT FROM (selected->>'source_review_revision')::integer OR source_review IS NULL
            OR NOT EXISTS(SELECT 1 FROM catalogue_translation_reviews WHERE sku=item AND locale='nl' AND translation_revision=source.revision AND revision=source_review AND decision='approved') THEN RETURN false; END IF;
    END IF;
    RETURN true;
END;
$$;
CREATE FUNCTION guard_publication_history() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE previous integer; family text; selected jsonb; route text;
BEGIN
    IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Publication history is immutable' USING ERRCODE='23514'; END IF;
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM catalogue_items WHERE sku=NEW.sku AND NOT fixture) THEN RAISE EXCEPTION 'Publication requires a merchant variant' USING ERRCODE='23514'; END IF;
    SELECT coalesce(max(revision),0) INTO previous FROM catalogue_publication_revisions WHERE sku=NEW.sku AND locale=NEW.locale;
    IF previous=2147483647 OR NEW.revision<>previous+1 THEN RAISE EXCEPTION 'Publication revision must follow current history' USING ERRCODE='23514'; END IF;
    IF NEW.choice->>'action'='withdraw' THEN
        IF NEW.choice-'action'<>'{}'::jsonb THEN RAISE EXCEPTION 'Withdrawal has no source selection' USING ERRCODE='23514'; END IF;
    ELSIF NEW.choice->>'action'='activate' THEN
        selected=NEW.choice->'selection';
        IF NEW.choice-ARRAY['action','selection']<>'{}'::jsonb OR jsonb_typeof(selected) IS DISTINCT FROM 'object'
            OR NOT (selected ?& ARRAY['facts','factual_review_revision','image','image_review_revision','translation_revision','translation_review_revision','source_review_revision','family_slug'])
            OR selected-ARRAY['facts','factual_review_revision','image','image_review_revision','translation_revision','translation_review_revision','source_review_revision','family_slug']<>'{}'::jsonb
            OR jsonb_typeof(selected->'facts') IS DISTINCT FROM 'object' OR jsonb_typeof(selected->'image') IS DISTINCT FROM 'object'
            OR NOT (selected->'facts' ?& ARRAY['bom_revision','specification_revision','price_revision'])
            OR (selected->'facts')-ARRAY['bom_revision','specification_revision','price_revision']<>'{}'::jsonb
            OR NOT (selected->'image' ?& ARRAY['asset_id','bom_revision','specification_revision','rights_revision'])
            OR (selected->'image')-ARRAY['asset_id','bom_revision','specification_revision','rights_revision']<>'{}'::jsonb
            OR jsonb_typeof(selected->'image'->'asset_id') IS DISTINCT FROM 'string'
            OR jsonb_typeof(selected->'family_slug') IS DISTINCT FROM 'string'
            OR length(selected->>'family_slug') NOT BETWEEN 1 AND 100 OR selected->>'family_slug' !~ '^[a-z0-9]+(-[a-z0-9]+)*$'
            OR EXISTS(SELECT 1 FROM jsonb_each(selected->'facts') e WHERE jsonb_typeof(e.value)<>'number' OR e.value::text !~ '^[1-9][0-9]*$')
            OR EXISTS(SELECT 1 FROM jsonb_each((selected->'image')-'asset_id') e WHERE jsonb_typeof(e.value)<>'number' OR e.value::text !~ '^[1-9][0-9]*$')
            OR EXISTS(SELECT 1 FROM jsonb_each(selected) e WHERE e.key IN ('factual_review_revision','image_review_revision','translation_revision','translation_review_revision') AND (jsonb_typeof(e.value)<>'number' OR e.value::text !~ '^[1-9][0-9]*$'))
            OR (selected->'source_review_revision'<>'null'::jsonb AND (jsonb_typeof(selected->'source_review_revision')<>'number' OR (selected->'source_review_revision')::text !~ '^[1-9][0-9]*$')) THEN RAISE EXCEPTION 'Closed publication selection required' USING ERRCODE='23514'; END IF;
        UPDATE catalogue_media_serialization SET anchor=anchor WHERE asset_id=selected->'image'->>'asset_id';
        IF NOT FOUND OR NOT publication_sources_current(NEW.sku,NEW.locale,NEW.choice,NEW.evaluated_at) THEN RAISE EXCEPTION 'Publication requires current approved sources and rights' USING ERRCODE='23514'; END IF;
        SELECT family_code INTO family FROM catalogue_variants WHERE sku=NEW.sku;
        INSERT INTO catalogue_family_routes(family_code,locale,slug,creation_effect_key) VALUES(family,NEW.locale,selected->>'family_slug',NEW.effect_key) ON CONFLICT DO NOTHING;
        SELECT slug INTO route FROM catalogue_family_routes WHERE family_code=family AND locale=NEW.locale;
        IF route IS DISTINCT FROM selected->>'family_slug' THEN RAISE EXCEPTION 'Family path already assigned' USING ERRCODE='23514'; END IF;
    ELSE RAISE EXCEPTION 'Unknown publication action' USING ERRCODE='23514'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_publication_history BEFORE INSERT OR UPDATE OR DELETE ON catalogue_publication_revisions FOR EACH ROW EXECUTE FUNCTION guard_publication_history();
CREATE TRIGGER guard_family_route_history BEFORE UPDATE OR DELETE ON catalogue_family_routes FOR EACH ROW EXECUTE FUNCTION guard_definition_history();
CREATE FUNCTION bind_family_route() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT publication_route_matches(NEW) THEN
        RAISE EXCEPTION 'Family path requires its activation event' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER bind_family_route AFTER INSERT ON catalogue_family_routes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION bind_family_route();
CREATE FUNCTION complete_publication_route() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.choice->>'action'='activate' AND NOT EXISTS(SELECT 1 FROM catalogue_family_routes r JOIN catalogue_variants v ON v.sku=NEW.sku AND v.family_code=r.family_code
        WHERE r.locale=NEW.locale AND r.slug=NEW.choice->'selection'->>'family_slug' AND publication_route_matches(r)) THEN
        RAISE EXCEPTION 'Activation requires its validated stable family path' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER complete_publication_route AFTER INSERT ON catalogue_publication_revisions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION complete_publication_route();
