-- Immutable copy/review history only; preview translations and publication stay separate.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CONSTRAINT staff_capabilities_capability_check
    CHECK(capability IN ('catalogue.manage','catalogue.review','warehouse.manage'));

CREATE TABLE catalogue_translation_revisions (
    sku text NOT NULL,
    locale text NOT NULL CHECK(locale IN ('nl','de','fr','en','ar','tr','kmr')),
    revision integer NOT NULL CHECK(revision>0),
    specification_revision integer NOT NULL,
    source_revision integer,
    source_locale text GENERATED ALWAYS AS ('nl'::text) STORED,
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    payload jsonb NOT NULL CHECK(jsonb_typeof(payload)='object' AND octet_length(payload::text)<=131072),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(sku,locale,revision),
    CONSTRAINT translation_specification_fk FOREIGN KEY(sku,specification_revision) REFERENCES catalogue_specification_revisions(sku,revision),
    CONSTRAINT translation_source_fk FOREIGN KEY(sku,source_locale,source_revision) REFERENCES catalogue_translation_revisions(sku,locale,revision),
    CONSTRAINT translation_source_shape CHECK((locale='nl' AND source_revision IS NULL) OR (locale<>'nl' AND source_revision IS NOT NULL AND source_revision>0))
);
CREATE TABLE catalogue_translation_reviews (
    sku text NOT NULL,
    locale text NOT NULL,
    translation_revision integer NOT NULL,
    revision integer NOT NULL CHECK(revision>0),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    decision text NOT NULL CHECK(decision IN ('approved','rejected')),
    evidence_ref text NOT NULL CHECK(length(btrim(evidence_ref)) BETWEEN 1 AND 500),
    qualification_ref text NOT NULL CHECK(length(btrim(qualification_ref)) BETWEEN 1 AND 500),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(sku,locale,translation_revision,revision),
    FOREIGN KEY(sku,locale,translation_revision) REFERENCES catalogue_translation_revisions(sku,locale,revision)
);
CREATE FUNCTION assert_translation_source(item text, language text, specification integer, source integer) RETURNS void LANGUAGE plpgsql AS $$
DECLARE current_specification integer; current_source integer; source_specification integer;
BEGIN
    SELECT max(revision) INTO current_specification FROM catalogue_specification_revisions WHERE sku=item;
    IF current_specification IS DISTINCT FROM specification THEN
        RAISE EXCEPTION 'Translation requires the current specification' USING ERRCODE='23514';
    END IF;
    IF language<>'nl' THEN
        SELECT revision,specification_revision INTO current_source,source_specification FROM catalogue_translation_revisions WHERE sku=item AND locale='nl' ORDER BY revision DESC LIMIT 1;
        IF current_source IS DISTINCT FROM source OR source_specification IS DISTINCT FROM specification THEN
            RAISE EXCEPTION 'Translation requires the current Dutch source' USING ERRCODE='23514';
        END IF;
    END IF;
END;
$$;
CREATE FUNCTION guard_translation_revision() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Translation history is immutable' USING ERRCODE='23514';
    END IF;
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    PERFORM assert_translation_source(NEW.sku,NEW.locale,NEW.specification_revision,NEW.source_revision);
    SELECT coalesce(max(revision),0) INTO current_revision FROM catalogue_translation_revisions WHERE sku=NEW.sku AND locale=NEW.locale;
    IF current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
        RAISE EXCEPTION 'Translation revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_translation_revision BEFORE INSERT OR UPDATE OR DELETE ON catalogue_translation_revisions FOR EACH ROW EXECUTE FUNCTION guard_translation_revision();
CREATE FUNCTION guard_translation_review() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE draft catalogue_translation_revisions%ROWTYPE; current_translation integer; current_review integer;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Translation review history is immutable' USING ERRCODE='23514';
    END IF;
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    SELECT * INTO draft FROM catalogue_translation_revisions WHERE sku=NEW.sku AND locale=NEW.locale AND revision=NEW.translation_revision;
    SELECT max(revision) INTO current_translation FROM catalogue_translation_revisions WHERE sku=NEW.sku AND locale=NEW.locale;
    IF draft.actor IS NULL OR NEW.actor=draft.actor THEN
        RAISE EXCEPTION 'Review requires an independent actor' USING ERRCODE='23514';
    END IF;
    IF NEW.decision='approved' THEN
        IF NEW.translation_revision IS DISTINCT FROM current_translation THEN
            RAISE EXCEPTION 'Approval requires current copy' USING ERRCODE='23514';
        END IF;
        PERFORM assert_translation_source(NEW.sku,NEW.locale,draft.specification_revision,draft.source_revision);
    END IF;
    SELECT coalesce(max(revision),0) INTO current_review FROM catalogue_translation_reviews WHERE sku=NEW.sku AND locale=NEW.locale AND translation_revision=NEW.translation_revision;
    IF current_review=2147483647 OR NEW.revision<>current_review+1 THEN
        RAISE EXCEPTION 'Review revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_translation_review BEFORE INSERT OR UPDATE OR DELETE ON catalogue_translation_reviews FOR EACH ROW EXECUTE FUNCTION guard_translation_review();
