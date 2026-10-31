-- Empty private promotion configuration. No coupons, staff grants or budgets are spent.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CHECK(capability IN
    ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review','promotion.manage','promotion.review'));
CREATE TABLE promotion_policy_anchors (
    id text PRIMARY KEY CHECK(id ~ '^[A-Z0-9][A-Z0-9-]{0,64}$' AND id<>'DEMO' AND id !~ '^DEMO-' AND id ~ '[A-Z0-9]$')
);
CREATE FUNCTION guard_promotion_anchor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR NEW IS DISTINCT FROM OLD THEN
        RAISE EXCEPTION 'Promotion identity is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_promotion_anchor BEFORE UPDATE OR DELETE ON promotion_policy_anchors
FOR EACH ROW EXECUTE FUNCTION guard_promotion_anchor();
CREATE TABLE promotion_policy_drafts (
    id text NOT NULL REFERENCES promotion_policy_anchors(id),
    revision integer NOT NULL CHECK(revision>0),
    policy jsonb NOT NULL CHECK(jsonb_typeof(policy)='object' AND octet_length(policy::text)<=32768),
    effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
    actor text NOT NULL REFERENCES staff_accounts(id),
    reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(id,revision)
);
CREATE TABLE promotion_policy_reviews (
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
    FOREIGN KEY(id,draft_revision) REFERENCES promotion_policy_drafts(id,revision)
);
CREATE FUNCTION guard_promotion_history() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_revision integer; draft_actor text;
BEGIN
    IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Promotion history is immutable' USING ERRCODE='23514'; END IF;
    -- A real write rejects a stale repeatable-read import after a competing revision.
    UPDATE promotion_policy_anchors SET id=id WHERE id=NEW.id;
    IF TG_TABLE_NAME='promotion_policy_drafts' THEN
        IF NEW.policy->'currency' IS DISTINCT FROM '"EUR"'::jsonb
            OR NEW.policy->'promotion'->>'id' IS DISTINCT FROM NEW.id
            OR NEW.policy->'promotion'->>'revision' IS DISTINCT FROM NEW.revision::text THEN
            RAISE EXCEPTION 'Promotion terms require exact identity' USING ERRCODE='23514';
        END IF;
        SELECT coalesce(max(revision),0) INTO current_revision FROM promotion_policy_drafts WHERE id=NEW.id;
    ELSE
        SELECT actor INTO draft_actor FROM promotion_policy_drafts WHERE id=NEW.id AND revision=NEW.draft_revision;
        IF draft_actor IS NULL OR draft_actor=NEW.actor THEN
            RAISE EXCEPTION 'Promotion review requires independent author' USING ERRCODE='23514';
        END IF;
        IF NEW.decision='approved' AND NEW.draft_revision<>(SELECT max(revision) FROM promotion_policy_drafts WHERE id=NEW.id) THEN
            RAISE EXCEPTION 'Promotion approval requires current draft' USING ERRCODE='23514';
        END IF;
        SELECT coalesce(max(revision),0) INTO current_revision FROM promotion_policy_reviews WHERE id=NEW.id AND draft_revision=NEW.draft_revision;
    END IF;
    IF current_revision=2147483647 OR NEW.revision<>current_revision+1 THEN
        RAISE EXCEPTION 'Promotion revision must follow current history' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_promotion_draft BEFORE INSERT OR UPDATE OR DELETE ON promotion_policy_drafts FOR EACH ROW EXECUTE FUNCTION guard_promotion_history();
CREATE TRIGGER guard_promotion_review BEFORE INSERT OR UPDATE OR DELETE ON promotion_policy_reviews FOR EACH ROW EXECUTE FUNCTION guard_promotion_history();
