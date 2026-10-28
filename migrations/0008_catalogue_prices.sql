-- Immutable draft price history. This never changes the fixture/public read model.
CREATE TABLE catalogue_price_revisions (
    sku text NOT NULL REFERENCES catalogue_variants(sku),
    revision integer NOT NULL CHECK (revision>0),
    effect_key text UNIQUE NOT NULL CHECK (effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    minor_units bigint NOT NULL CHECK (minor_units>=0),
    currency text NOT NULL DEFAULT 'EUR' CHECK (currency='EUR'),
    evidence_ref text NOT NULL CHECK (length(btrim(evidence_ref)) BETWEEN 1 AND 512),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (sku,revision)
);
CREATE FUNCTION guard_catalogue_price_revision() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Price history is immutable' USING ERRCODE='23514';
    END IF;
    -- Serialize every writer on the same variant, including direct imports.
    -- A no-op UPDATE creates an MVCC conflict for stale repeatable-read writers.
    UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=NEW.sku;
    SELECT coalesce(max(revision),0) INTO current_revision
        FROM catalogue_price_revisions WHERE sku=NEW.sku;
    IF current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
        RAISE EXCEPTION 'Price revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_catalogue_price_revision BEFORE INSERT OR UPDATE OR DELETE
ON catalogue_price_revisions FOR EACH ROW EXECUTE FUNCTION guard_catalogue_price_revision();
