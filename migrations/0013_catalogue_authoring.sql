-- Private merchant definitions. No prices, stock, facts or publication are activated.
CREATE TABLE catalogue_definition_events (
    effect_key text PRIMARY KEY CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    payload jsonb NOT NULL CHECK(jsonb_typeof(payload)='object' AND octet_length(payload::text)<=131072),
    bom_revision integer,
    actor text NOT NULL REFERENCES staff_accounts(id),
    source_ref text NOT NULL CHECK(length(btrim(source_ref)) BETWEEN 1 AND 500),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK(payload->>'kind' IS NOT NULL AND payload->>'kind' IN ('family','component','variant','bom')),
    CHECK((payload->>'kind' IN ('family','component') AND bom_revision IS NULL)
        OR (payload->>'kind'='variant' AND bom_revision IS NOT NULL AND bom_revision=1)
        OR (payload->>'kind'='bom' AND bom_revision IS NOT NULL AND bom_revision>1))
);
CREATE FUNCTION guard_definition_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Definition history is immutable' USING ERRCODE='23514'; END;
$$;
CREATE TRIGGER guard_definition_history BEFORE UPDATE OR DELETE ON catalogue_definition_events FOR EACH ROW EXECUTE FUNCTION guard_definition_history();

ALTER TABLE product_families DROP CONSTRAINT product_families_code_check, DROP CONSTRAINT product_families_fixture_check;
ALTER TABLE product_families ADD COLUMN creation_effect_key text REFERENCES catalogue_definition_events(effect_key);
ALTER TABLE product_families ADD CONSTRAINT family_identity_class CHECK(
    (fixture AND code ~ '^DEMO-[A-Z0-9-]{1,60}$' AND creation_effect_key IS NULL) OR
    (NOT fixture AND code ~ '^[A-Z0-9]([A-Z0-9-]{0,63}[A-Z0-9])?$' AND code<>'DEMO' AND code NOT LIKE 'DEMO-%' AND creation_effect_key IS NOT NULL));
ALTER TABLE physical_components DROP CONSTRAINT physical_components_code_check, DROP CONSTRAINT physical_components_fixture_check;
ALTER TABLE physical_components ADD COLUMN creation_effect_key text REFERENCES catalogue_definition_events(effect_key);
ALTER TABLE physical_components ADD CONSTRAINT component_identity_class CHECK(
    (fixture AND code ~ '^DEMO-[A-Z0-9-]{1,60}$' AND creation_effect_key IS NULL) OR
    (NOT fixture AND code ~ '^[A-Z0-9]([A-Z0-9-]{0,63}[A-Z0-9])?$' AND code<>'DEMO' AND code NOT LIKE 'DEMO-%' AND creation_effect_key IS NOT NULL));
ALTER TABLE catalogue_items DROP CONSTRAINT catalogue_items_sku_check, DROP CONSTRAINT catalogue_items_fixture_check;
ALTER TABLE catalogue_items ALTER COLUMN minor_units DROP NOT NULL, ALTER COLUMN slug DROP NOT NULL;
ALTER TABLE catalogue_items ADD COLUMN creation_effect_key text REFERENCES catalogue_definition_events(effect_key);
ALTER TABLE catalogue_items ADD CONSTRAINT item_identity_class CHECK(
    (fixture AND sku ~ '^DEMO-[A-Z0-9-]{1,60}$' AND creation_effect_key IS NULL) OR
    (NOT fixture AND sku ~ '^[A-Z0-9]([A-Z0-9-]{0,63}[A-Z0-9])?$' AND sku<>'DEMO' AND sku NOT LIKE 'DEMO-%' AND creation_effect_key IS NOT NULL));
-- These fields belong to the legacy fixture projection. Merchant prices and URLs
-- must come from future approved publication snapshots, never placeholders here.
ALTER TABLE catalogue_items ADD CONSTRAINT item_legacy_preview_fields CHECK(
    (fixture AND minor_units IS NOT NULL AND slug IS NOT NULL) OR
    (NOT fixture AND minor_units IS NULL AND slug IS NULL));
ALTER TABLE catalogue_boms ADD COLUMN creation_effect_key text REFERENCES catalogue_definition_events(effect_key);

CREATE FUNCTION bind_definition_creation() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE command jsonb;
BEGIN
    IF NEW.fixture THEN RETURN NEW; END IF;
    SELECT payload INTO command FROM catalogue_definition_events WHERE effect_key=NEW.creation_effect_key;
    IF command->>'kind' IS DISTINCT FROM TG_ARGV[0] OR command->>TG_ARGV[1] IS DISTINCT FROM to_jsonb(NEW)->>TG_ARGV[1] THEN
        RAISE EXCEPTION 'Definition requires its exact creation event' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER bind_family_creation BEFORE INSERT ON product_families FOR EACH ROW EXECUTE FUNCTION bind_definition_creation('family','code');
CREATE TRIGGER bind_component_creation BEFORE INSERT ON physical_components FOR EACH ROW EXECUTE FUNCTION bind_definition_creation('component','code');
CREATE TRIGGER bind_item_creation BEFORE INSERT ON catalogue_items FOR EACH ROW EXECUTE FUNCTION bind_definition_creation('variant','sku');
CREATE FUNCTION guard_family_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.code,NEW.fixture,NEW.creation_effect_key) IS DISTINCT FROM (OLD.code,OLD.fixture,OLD.creation_effect_key) THEN
        RAISE EXCEPTION 'Family identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_family_identity BEFORE UPDATE ON product_families FOR EACH ROW EXECUTE FUNCTION guard_family_identity();
CREATE OR REPLACE FUNCTION guard_component_kind() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.code,NEW.kind,NEW.fixture,NEW.creation_effect_key) IS DISTINCT FROM (OLD.code,OLD.kind,OLD.fixture,OLD.creation_effect_key) THEN
        RAISE EXCEPTION 'Physical component identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE FUNCTION guard_item_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.sku,NEW.fixture,NEW.currency,NEW.creation_effect_key) IS DISTINCT FROM (OLD.sku,OLD.fixture,OLD.currency,OLD.creation_effect_key) THEN
        RAISE EXCEPTION 'Item identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_item_identity BEFORE UPDATE ON catalogue_items FOR EACH ROW EXECUTE FUNCTION guard_item_identity();
CREATE FUNCTION guard_definition_deletion() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT OLD.fixture THEN RAISE EXCEPTION 'Merchant definitions retain their history' USING ERRCODE='23514'; END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER guard_family_deletion BEFORE DELETE ON product_families FOR EACH ROW EXECUTE FUNCTION guard_definition_deletion();
CREATE TRIGGER guard_component_deletion BEFORE DELETE ON physical_components FOR EACH ROW EXECUTE FUNCTION guard_definition_deletion();
CREATE TRIGGER guard_item_deletion BEFORE DELETE ON catalogue_items FOR EACH ROW EXECUTE FUNCTION guard_definition_deletion();
CREATE FUNCTION guard_variant_definition() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE item_fixture boolean; family_fixture boolean;
BEGIN
    IF TG_OP='UPDATE' AND (NEW.sku,NEW.family_code,NEW.size_code,NEW.textile_code,NEW.stock_policy) IS DISTINCT FROM (OLD.sku,OLD.family_code,OLD.size_code,OLD.textile_code,OLD.stock_policy) THEN
        RAISE EXCEPTION 'Variant identity is immutable' USING ERRCODE='23514';
    END IF;
    SELECT fixture INTO item_fixture FROM catalogue_items WHERE sku=NEW.sku;
    SELECT fixture INTO family_fixture FROM product_families WHERE code=NEW.family_code;
    IF item_fixture IS DISTINCT FROM family_fixture THEN RAISE EXCEPTION 'Variant fixture classes cannot mix' USING ERRCODE='23514'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_variant_definition BEFORE INSERT OR UPDATE ON catalogue_variants FOR EACH ROW EXECUTE FUNCTION guard_variant_definition();
CREATE FUNCTION guard_legacy_preview_copy() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM catalogue_items WHERE sku=NEW.sku AND fixture) THEN
        RAISE EXCEPTION 'Legacy copy is fixture-only' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_legacy_preview_copy BEFORE INSERT OR UPDATE ON catalogue_translations FOR EACH ROW EXECUTE FUNCTION guard_legacy_preview_copy();

CREATE FUNCTION bind_bom_definition() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE item_fixture boolean; command jsonb; assigned integer; expected integer; current_revision integer;
BEGIN
    SELECT fixture INTO item_fixture FROM catalogue_items WHERE sku=NEW.sku;
    IF item_fixture THEN
        IF NEW.creation_effect_key IS NOT NULL THEN RAISE EXCEPTION 'Fixtures cannot adopt merchant events' USING ERRCODE='23514'; END IF;
        RETURN NEW;
    END IF;
    IF TG_OP='UPDATE' AND NEW.creation_effect_key IS DISTINCT FROM OLD.creation_effect_key THEN
        RAISE EXCEPTION 'BOM creation identity is immutable' USING ERRCODE='23514';
    END IF;
    SELECT payload,bom_revision INTO command,assigned FROM catalogue_definition_events WHERE effect_key=NEW.creation_effect_key;
    IF command->>'kind' IS NULL OR command->>'kind' NOT IN ('variant','bom') OR command->>'sku' IS DISTINCT FROM NEW.sku OR assigned IS DISTINCT FROM NEW.revision THEN
        RAISE EXCEPTION 'BOM requires its exact creation event' USING ERRCODE='23514';
    END IF;
    IF TG_OP='INSERT' THEN
        UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
        expected=CASE WHEN command->>'kind'='variant' THEN 0 ELSE (command->>'expected_revision')::integer END;
        SELECT coalesce(max(revision),0) INTO current_revision FROM catalogue_boms WHERE sku=NEW.sku;
        IF expected IS DISTINCT FROM current_revision OR current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
            RAISE EXCEPTION 'BOM revision must follow current history' USING ERRCODE='23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER bind_bom_definition BEFORE INSERT OR UPDATE ON catalogue_boms FOR EACH ROW EXECUTE FUNCTION bind_bom_definition();
CREATE OR REPLACE FUNCTION guard_bom_revision() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE item_fill text; item_fixture boolean; covers bigint; foams bigint;
BEGIN
    IF TG_OP<>'INSERT' THEN
        IF OLD.sealed THEN RAISE EXCEPTION 'Sealed BOM is immutable' USING ERRCODE='23514'; END IF;
        IF TG_OP='DELETE' THEN RETURN OLD; END IF;
        IF (NEW.sku,NEW.revision) IS DISTINCT FROM (OLD.sku,OLD.revision) THEN RAISE EXCEPTION 'BOM identity is immutable' USING ERRCODE='23514'; END IF;
    END IF;
    IF NEW.sealed THEN
        UPDATE catalogue_items SET fill=fill WHERE sku=NEW.sku RETURNING fill,fixture INTO item_fill,item_fixture;
        SELECT count(*) FILTER(WHERE c.kind='cover'),count(*) FILTER(WHERE c.kind='foam') INTO covers,foams
        FROM catalogue_bom_lines l JOIN physical_components c ON c.code=l.component_code WHERE l.sku=NEW.sku AND l.revision=NEW.revision;
        IF covers=0 OR covers+foams>64 OR (item_fill='covers-only' AND foams<>0) OR (item_fill='with-foam' AND foams=0)
            OR EXISTS(SELECT 1 FROM catalogue_bom_lines l JOIN physical_components c ON c.code=l.component_code WHERE l.sku=NEW.sku AND l.revision=NEW.revision AND c.fixture IS DISTINCT FROM item_fixture) THEN
            RAISE EXCEPTION 'BOM does not match variant configuration/class' USING ERRCODE='23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE FUNCTION definition_event_matches(candidate catalogue_definition_events) RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE command_kind text; matched boolean; actual_lines jsonb; expected_lines jsonb;
BEGIN
    command_kind=candidate.payload->>'kind';
    CASE command_kind
    WHEN 'family' THEN SELECT EXISTS(SELECT 1 FROM product_families WHERE code=candidate.payload->>'code' AND NOT fixture AND creation_effect_key=candidate.effect_key) INTO matched;
    WHEN 'component' THEN SELECT EXISTS(SELECT 1 FROM physical_components WHERE code=candidate.payload->>'code' AND kind=candidate.payload->>'component_kind' AND NOT fixture AND creation_effect_key=candidate.effect_key) INTO matched;
    WHEN 'variant' THEN SELECT EXISTS(SELECT 1 FROM catalogue_items i JOIN catalogue_variants v USING(sku) JOIN product_families f ON f.code=v.family_code WHERE i.sku=candidate.payload->>'sku' AND NOT i.fixture AND NOT f.fixture AND i.creation_effect_key=candidate.effect_key AND i.fill=candidate.payload->>'fill' AND v.family_code=candidate.payload->>'family_code' AND v.size_code=candidate.payload->>'size_code' AND v.textile_code=candidate.payload->>'textile_code') INTO matched;
    WHEN 'bom' THEN matched=true;
    ELSE matched=false;
    END CASE;
    IF command_kind IN ('variant','bom') THEN
        matched=matched AND EXISTS(SELECT 1 FROM catalogue_boms b JOIN catalogue_items i USING(sku) WHERE b.sku=candidate.payload->>'sku' AND NOT i.fixture AND b.revision=candidate.bom_revision AND b.sealed AND b.creation_effect_key=candidate.effect_key)
            AND NOT EXISTS(SELECT 1 FROM catalogue_bom_lines l JOIN physical_components c ON c.code=l.component_code WHERE l.sku=candidate.payload->>'sku' AND l.revision=candidate.bom_revision AND c.fixture);
        SELECT jsonb_agg(jsonb_build_object('component_code',component_code,'quantity',quantity) ORDER BY component_code) INTO actual_lines FROM catalogue_bom_lines WHERE sku=candidate.payload->>'sku' AND revision=candidate.bom_revision;
        SELECT jsonb_agg(value ORDER BY value->>'component_code') INTO expected_lines FROM jsonb_array_elements(candidate.payload->'lines');
        matched=matched AND actual_lines IS NOT DISTINCT FROM expected_lines;
    END IF;
    RETURN matched;
END;
$$;
CREATE FUNCTION complete_definition_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT definition_event_matches(NEW) THEN RAISE EXCEPTION 'Definition event requires its complete matching entity' USING ERRCODE='23514'; END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER complete_definition_event AFTER INSERT ON catalogue_definition_events DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION complete_definition_event();
