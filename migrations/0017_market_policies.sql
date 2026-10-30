-- No policies or grants are seeded. Exact postcodes never imply territorial coverage.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CHECK(capability IN
    ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review'));
CREATE TABLE market_policy_anchors (
    id text PRIMARY KEY CHECK(id ~ '^[A-Z0-9][A-Z0-9-]{0,64}$' AND id<>'DEMO' AND id !~ '^DEMO-' AND id ~ '[A-Z0-9]$'),
    country text NOT NULL CHECK(country IN ('AT','BE','BG','HR','CY','CZ','DK','EE','FI','FR','DE','GR','HU','IE','IT','LV','LT','LU','MT','NL','PL','PT','RO','SK','SI','ES','SE')),
    postcode text NOT NULL CHECK(length(postcode) BETWEEN 1 AND 16 AND postcode=btrim(postcode)
        AND postcode ~ '^[A-Z0-9 -]+$' AND postcode ~ '[A-Z0-9]'),
    UNIQUE(country,postcode)
);
CREATE FUNCTION guard_market_anchor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'Market identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_market_anchor BEFORE UPDATE OR DELETE ON market_policy_anchors
FOR EACH ROW EXECUTE FUNCTION guard_market_anchor();
CREATE TABLE market_policy_drafts (
    id text NOT NULL REFERENCES market_policy_anchors(id),
    revision integer NOT NULL CHECK(revision>0),
    policy jsonb NOT NULL CHECK(jsonb_typeof(policy)='object' AND octet_length(policy::text)<=65536),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(id,revision)
);
CREATE TABLE market_policy_reviews (
    id text NOT NULL,
    draft_revision integer NOT NULL,
    revision integer NOT NULL CHECK(revision>0),
    decision text NOT NULL CHECK(decision IN ('approved','rejected')),
    evidence_ref text NOT NULL CHECK(length(btrim(evidence_ref)) BETWEEN 1 AND 500),
    qualification_ref text NOT NULL CHECK(length(btrim(qualification_ref)) BETWEEN 1 AND 500),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(id,draft_revision,revision),
    FOREIGN KEY(id,draft_revision) REFERENCES market_policy_drafts(id,revision)
);
CREATE FUNCTION guard_market_history() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer; draft_actor text; anchor market_policy_anchors%ROWTYPE;
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Market history is immutable' USING ERRCODE='23514';
    END IF;
    -- A write, rather than only a row lock, rejects stale repeatable-read imports.
    UPDATE market_policy_anchors SET id=id WHERE id=NEW.id RETURNING * INTO anchor;
    IF TG_TABLE_NAME='market_policy_drafts' THEN
        SELECT coalesce(max(revision),0) INTO current_revision FROM market_policy_drafts WHERE id=NEW.id;
        IF NEW.policy->>'id' IS DISTINCT FROM NEW.id
            OR NEW.policy->>'revision' IS DISTINCT FROM NEW.revision::text
            OR NEW.policy->'destination'->>'country' IS DISTINCT FROM anchor.country
            OR NEW.policy->'destination'->>'postcode' IS DISTINCT FROM anchor.postcode THEN
            RAISE EXCEPTION 'Policy identity must match immutable anchor' USING ERRCODE='23514';
        END IF;
    ELSE
        SELECT actor INTO draft_actor FROM market_policy_drafts WHERE id=NEW.id AND revision=NEW.draft_revision;
        IF draft_actor IS NULL OR draft_actor=NEW.actor THEN
            RAISE EXCEPTION 'Market review requires an independent author' USING ERRCODE='23514';
        END IF;
        IF NEW.decision='approved' AND NEW.draft_revision<>(SELECT max(revision) FROM market_policy_drafts WHERE id=NEW.id) THEN
            RAISE EXCEPTION 'Approval requires current policy' USING ERRCODE='23514';
        END IF;
        SELECT coalesce(max(revision),0) INTO current_revision FROM market_policy_reviews WHERE id=NEW.id AND draft_revision=NEW.draft_revision;
    END IF;
    IF current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
        RAISE EXCEPTION 'Market revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_market_draft BEFORE INSERT OR UPDATE OR DELETE ON market_policy_drafts
FOR EACH ROW EXECUTE FUNCTION guard_market_history();
CREATE TRIGGER guard_market_review BEFORE INSERT OR UPDATE OR DELETE ON market_policy_reviews
FOR EACH ROW EXECUTE FUNCTION guard_market_history();
