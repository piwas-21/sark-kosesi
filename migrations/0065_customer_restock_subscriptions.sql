-- Current private opt-in only: this table does not authorize or record delivery.
CREATE TABLE customer_restock_buckets (
    account_id bigint PRIMARY KEY REFERENCES verified_customer_accounts(id),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0)
);
CREATE TABLE customer_restock_subscriptions (
    account_id bigint NOT NULL REFERENCES customer_restock_buckets(account_id),
    sku text NOT NULL REFERENCES catalogue_variants(sku),
    generation bigint NOT NULL CHECK (generation > 0),
    opted_in_at bigint NOT NULL CHECK (opted_in_at >= 0),
    PRIMARY KEY (account_id, sku),
    UNIQUE (account_id, generation)
);

CREATE FUNCTION guard_restock_bucket() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Restock custody requires read committed' USING ERRCODE='23514';
    END IF;
    IF TG_OP = 'INSERT' THEN
        IF NEW.revision <> 0 THEN RAISE EXCEPTION 'Invalid restock revision' USING ERRCODE='23514'; END IF;
        RETURN NEW;
    END IF;
    IF TG_OP = 'DELETE' OR pg_trigger_depth() < 2 OR NEW.account_id <> OLD.account_id
       OR NEW.revision <> OLD.revision + 1 THEN
        RAISE EXCEPTION 'Invalid restock bucket mutation' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_restock_bucket BEFORE INSERT OR UPDATE OR DELETE ON customer_restock_buckets
FOR EACH ROW EXECUTE FUNCTION guard_restock_bucket();

CREATE FUNCTION guard_restock_subscription() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owner_id bigint; current_revision bigint; prior_generation bigint;
BEGIN
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Restock custody requires read committed' USING ERRCODE='23514';
    END IF;
    IF TG_OP = 'UPDATE' THEN RAISE EXCEPTION 'Restock subscription is immutable' USING ERRCODE='23514'; END IF;
    owner_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.account_id ELSE NEW.account_id END;
    SELECT revision INTO current_revision FROM customer_restock_buckets WHERE account_id=owner_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Missing restock bucket' USING ERRCODE='23503'; END IF;
    IF TG_OP = 'INSERT' THEN
        IF NEW.generation IS NOT NULL OR NOT EXISTS (SELECT 1 FROM catalogue_items WHERE sku=NEW.sku AND NOT fixture) THEN
            RAISE EXCEPTION 'Invalid restock subscription' USING ERRCODE='23514';
        END IF;
        SELECT generation INTO prior_generation FROM customer_restock_subscriptions WHERE account_id=owner_id AND sku=NEW.sku;
        IF FOUND THEN NEW.generation := prior_generation; RETURN NEW; END IF;
        IF (SELECT count(*) FROM customer_restock_subscriptions WHERE account_id=owner_id) >= 25 THEN
            RAISE EXCEPTION 'Restock capacity exceeded' USING ERRCODE='23514';
        END IF;
        NEW.generation := current_revision + 1;
    END IF;
    UPDATE customer_restock_buckets SET revision=revision+1 WHERE account_id=owner_id;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END $$;
CREATE TRIGGER guard_restock_subscription BEFORE INSERT OR UPDATE OR DELETE ON customer_restock_subscriptions
FOR EACH ROW EXECUTE FUNCTION guard_restock_subscription();
