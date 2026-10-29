-- Independent fact attestations, never public activation or evidence verification.
CREATE TABLE catalogue_factual_reviews (
    sku text NOT NULL,
    bom_revision integer NOT NULL,
    specification_revision integer NOT NULL,
    price_revision integer NOT NULL,
    revision integer NOT NULL CHECK(revision>0),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    decision text NOT NULL CHECK(decision IN ('approved','rejected')),
    evidence_ref text NOT NULL CHECK(length(btrim(evidence_ref)) BETWEEN 1 AND 500),
    qualification_ref text NOT NULL CHECK(length(btrim(qualification_ref)) BETWEEN 1 AND 500),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(sku,bom_revision,specification_revision,price_revision,revision),
    FOREIGN KEY(sku,bom_revision) REFERENCES catalogue_boms(sku,revision),
    FOREIGN KEY(sku,specification_revision) REFERENCES catalogue_specification_revisions(sku,revision),
    FOREIGN KEY(sku,price_revision) REFERENCES catalogue_price_revisions(sku,revision)
);
CREATE FUNCTION guard_factual_review() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_review integer; specification catalogue_specification_revisions%ROWTYPE;
    price_actor text; bom_actor text; identity_actor text; current_bom integer; current_specification integer; current_price integer;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Factual review history is immutable' USING ERRCODE='23514';
    END IF;
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    SELECT * INTO specification FROM catalogue_specification_revisions WHERE sku=NEW.sku AND revision=NEW.specification_revision;
    SELECT actor INTO price_actor FROM catalogue_price_revisions WHERE sku=NEW.sku AND revision=NEW.price_revision;
    SELECT e.actor INTO bom_actor FROM catalogue_boms b JOIN catalogue_definition_events e ON e.effect_key=b.creation_effect_key
        WHERE b.sku=NEW.sku AND b.revision=NEW.bom_revision AND b.sealed AND definition_event_matches(e);
    SELECT e.actor INTO identity_actor FROM catalogue_items i JOIN catalogue_definition_events e ON e.effect_key=i.creation_effect_key
        WHERE i.sku=NEW.sku AND NOT i.fixture AND definition_event_matches(e);
    IF NOT EXISTS(SELECT 1 FROM catalogue_items WHERE sku=NEW.sku AND NOT fixture)
        OR specification.bom_revision IS DISTINCT FROM NEW.bom_revision OR price_actor IS NULL OR bom_actor IS NULL OR identity_actor IS NULL
        OR NEW.actor IN (specification.actor,price_actor,bom_actor,identity_actor) THEN
        RAISE EXCEPTION 'Facts require merchant snapshots and an independent reviewer' USING ERRCODE='23514';
    END IF;
    IF NEW.decision='approved' THEN
        SELECT max(revision) INTO current_bom FROM catalogue_boms WHERE sku=NEW.sku AND sealed;
        SELECT max(revision) INTO current_specification FROM catalogue_specification_revisions WHERE sku=NEW.sku;
        SELECT max(revision) INTO current_price FROM catalogue_price_revisions WHERE sku=NEW.sku;
        IF (NEW.bom_revision,NEW.specification_revision,NEW.price_revision) IS DISTINCT FROM (current_bom,current_specification,current_price) THEN
            RAISE EXCEPTION 'Approval requires current facts' USING ERRCODE='23514';
        END IF;
    END IF;
    SELECT coalesce(max(revision),0) INTO current_review FROM catalogue_factual_reviews
        WHERE sku=NEW.sku AND bom_revision=NEW.bom_revision AND specification_revision=NEW.specification_revision AND price_revision=NEW.price_revision;
    IF current_review=2147483647 OR NEW.revision<>current_review+1 THEN
        RAISE EXCEPTION 'Factual review revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_factual_review BEFORE INSERT OR UPDATE OR DELETE ON catalogue_factual_reviews FOR EACH ROW EXECUTE FUNCTION guard_factual_review();
