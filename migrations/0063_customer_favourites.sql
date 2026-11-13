-- Private account membership stores stable merchant identities only. Publication
-- and session authority are evaluated by the application while claims are held.
CREATE TABLE customer_favourite_buckets (
    account_id bigint PRIMARY KEY REFERENCES verified_customer_accounts(id),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0)
);
CREATE TABLE customer_favourites (
    account_id bigint NOT NULL REFERENCES customer_favourite_buckets(account_id),
    sku text NOT NULL REFERENCES catalogue_variants(sku),
    PRIMARY KEY (account_id, sku)
);

CREATE FUNCTION guard_favourite_bucket() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.revision <> 0 THEN RAISE EXCEPTION 'Invalid favourite revision' USING ERRCODE='23514'; END IF;
        RETURN NEW;
    END IF;
    IF TG_OP = 'DELETE' OR pg_trigger_depth() < 2 OR NEW.account_id <> OLD.account_id
       OR NEW.revision <> OLD.revision + 1 THEN
        RAISE EXCEPTION 'Invalid favourite bucket mutation' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER guard_favourite_bucket BEFORE INSERT OR UPDATE OR DELETE ON customer_favourite_buckets
FOR EACH ROW EXECUTE FUNCTION guard_favourite_bucket();

CREATE FUNCTION guard_favourite_membership() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owner_id bigint; existing boolean;
BEGIN
    IF TG_OP = 'UPDATE' THEN RAISE EXCEPTION 'Favourite identity is immutable' USING ERRCODE='23514'; END IF;
    owner_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.account_id ELSE NEW.account_id END;
    PERFORM 1 FROM customer_favourite_buckets WHERE account_id=owner_id FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Missing favourite bucket' USING ERRCODE='23503'; END IF;
    IF TG_OP = 'INSERT' THEN
        IF NOT EXISTS (SELECT 1 FROM catalogue_items WHERE sku=NEW.sku AND NOT fixture) THEN
            RAISE EXCEPTION 'Invalid favourite identity' USING ERRCODE='23514';
        END IF;
        SELECT EXISTS (SELECT 1 FROM customer_favourites WHERE account_id=owner_id AND sku=NEW.sku) INTO existing;
        IF existing THEN RETURN NEW; END IF;
        IF (SELECT count(*) FROM customer_favourites WHERE account_id=owner_id) >= 25 THEN
            RAISE EXCEPTION 'Favourite capacity exceeded' USING ERRCODE='23514';
        END IF;
    END IF;
    UPDATE customer_favourite_buckets SET revision=revision+1 WHERE account_id=owner_id;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END $$;
CREATE TRIGGER guard_favourite_membership BEFORE INSERT OR UPDATE OR DELETE ON customer_favourites
FOR EACH ROW EXECUTE FUNCTION guard_favourite_membership();
