-- Draft evidence snapshots. No production product facts or publishing path.
CREATE TABLE catalogue_specification_revisions (
    sku text NOT NULL,
    revision integer NOT NULL CHECK (revision>0),
    bom_revision integer NOT NULL,
    effect_key text UNIQUE NOT NULL CHECK (effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload)='object' AND octet_length(payload::text)<=131072),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (sku,revision),
    FOREIGN KEY (sku,bom_revision) REFERENCES catalogue_boms(sku,revision)
);
CREATE FUNCTION guard_catalogue_specification_revision() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Specification history is immutable' USING ERRCODE='23514';
    END IF;
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    IF NOT EXISTS (SELECT 1 FROM catalogue_boms WHERE sku=NEW.sku AND revision=NEW.bom_revision AND sealed) THEN
        RAISE EXCEPTION 'Specifications require a sealed BOM' USING ERRCODE='23514';
    END IF;
    SELECT coalesce(max(revision),0) INTO current_revision FROM catalogue_specification_revisions WHERE sku=NEW.sku;
    IF current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
        RAISE EXCEPTION 'Specification revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_catalogue_specification_revision BEFORE INSERT OR UPDATE OR DELETE
ON catalogue_specification_revisions FOR EACH ROW EXECUTE FUNCTION guard_catalogue_specification_revision();
