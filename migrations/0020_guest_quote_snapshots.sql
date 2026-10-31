-- Empty owned indicative quotes. No order, policy, stock or payment authority is seeded.
CREATE TABLE guest_quote_snapshots (
    id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
    basket_id text NOT NULL,
    basket_revision integer NOT NULL CHECK(basket_revision>0),
    effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    locale text NOT NULL CHECK(locale IN ('nl','de','fr','en','ar','tr','kmr')),
    country text NOT NULL,
    postcode text NOT NULL,
    policy_id text NOT NULL,
    policy_revision integer NOT NULL,
    policy_review_revision integer NOT NULL,
    created_at bigint NOT NULL CHECK(created_at>=0),
    expires_at bigint NOT NULL CHECK(expires_at>created_at AND expires_at<=9007199254740991 AND expires_at-created_at<=300),
    snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=32768),
    UNIQUE(basket_id,effect_key),
    FOREIGN KEY(basket_id,basket_revision) REFERENCES guest_basket_effects(basket_id,revision),
    FOREIGN KEY(policy_id,policy_revision,policy_review_revision) REFERENCES market_policy_reviews(id,draft_revision,revision)
);
CREATE INDEX guest_quote_snapshots_expiry ON guest_quote_snapshots(expires_at);
CREATE TABLE guest_quote_quota (
    basket_id text PRIMARY KEY REFERENCES guest_baskets(id),
    rate_window bigint NOT NULL CHECK(rate_window>=0),
    issued integer NOT NULL CHECK(issued BETWEEN 0 AND 10)
);
CREATE FUNCTION guard_guest_quote() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE binding jsonb; intent jsonb; original jsonb; item jsonb;
BEGIN
    IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Quote history is immutable' USING ERRCODE='23514'; END IF;
    binding:=jsonb_build_object('quote_id',NEW.id,'basket_id',NEW.basket_id,'basket_revision',NEW.basket_revision,'locale',NEW.locale,
        'country',NEW.country,'postcode',NEW.postcode,'policy_id',NEW.policy_id,'policy_revision',NEW.policy_revision,
        'policy_review_revision',NEW.policy_review_revision,'created_at',NEW.created_at,'expires_at',NEW.expires_at);
    IF NEW.snapshot->'binding' IS DISTINCT FROM binding OR NEW.snapshot->'schema_version' IS DISTINCT FROM '1'::jsonb
        OR NEW.snapshot->'currency' IS DISTINCT FROM '"EUR"'::jsonb
        OR jsonb_typeof(NEW.snapshot->'lines') IS DISTINCT FROM 'array' THEN
        RAISE EXCEPTION 'Quote requires its exact closed binding' USING ERRCODE='23514';
    END IF;
    IF jsonb_array_length(NEW.snapshot->'lines') NOT BETWEEN 1 AND 20
        OR NOT EXISTS(SELECT 1 FROM market_policy_reviews r JOIN market_policy_anchors a USING(id)
            WHERE r.id=NEW.policy_id AND r.draft_revision=NEW.policy_revision AND r.revision=NEW.policy_review_revision
                AND r.decision='approved' AND a.country=NEW.country AND a.postcode=NEW.postcode) THEN
        RAISE EXCEPTION 'Quote requires approved exact-destination source' USING ERRCODE='23514';
    END IF;
    SELECT jsonb_agg(jsonb_build_object('sku',l->>'sku','quantity',l->'quantity') ORDER BY (l->>'sku') COLLATE "C") INTO intent
        FROM jsonb_array_elements(NEW.snapshot->'lines') l;
    SELECT lines INTO original FROM guest_basket_effects WHERE basket_id=NEW.basket_id AND revision=NEW.basket_revision;
    IF intent IS DISTINCT FROM original THEN RAISE EXCEPTION 'Quote lines require exact immutable basket intent' USING ERRCODE='23514'; END IF;
    FOR item IN SELECT value FROM jsonb_array_elements(NEW.snapshot->'lines') LOOP
        IF NOT EXISTS(SELECT 1 FROM catalogue_publication_revisions p WHERE p.sku=item->>'sku' AND p.locale=NEW.locale
            AND p.revision=(item->>'publication_revision')::integer AND p.choice->>'action'='activate'
            AND (p.choice->'selection'->'facts'->>'bom_revision')::integer=(item->>'bom_revision')::integer) THEN
            RAISE EXCEPTION 'Quote requires exact published BOM source' USING ERRCODE='23514';
        END IF;
    END LOOP;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_guest_quote BEFORE INSERT OR UPDATE OR DELETE ON guest_quote_snapshots FOR EACH ROW EXECUTE FUNCTION guard_guest_quote();
