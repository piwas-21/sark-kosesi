-- Empty immutable reservation manifests; no stock, orders or selling authority seeded.
CREATE TABLE basket_hold_groups (
    id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
    owner_id text NOT NULL CHECK(owner_id ~ '^[a-f0-9]{32}$'),
    created_at bigint NOT NULL CHECK(created_at>=0),
    expires_at bigint NOT NULL CHECK(expires_at>created_at AND expires_at-created_at<=1800),
    members_count integer NOT NULL CHECK(members_count BETWEEN 1 AND 20),
    sealed boolean NOT NULL DEFAULT false
);
ALTER TABLE stock_holds ADD COLUMN basket_group_id text REFERENCES basket_hold_groups(id);
CREATE INDEX stock_holds_basket_group ON stock_holds(basket_group_id) WHERE basket_group_id IS NOT NULL;
CREATE FUNCTION guard_hold_basket_binding() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE principal text; started bigint; expiry bigint; closed boolean;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF NEW.basket_group_id IS DISTINCT FROM OLD.basket_group_id THEN
            RAISE EXCEPTION 'Hold basket binding is immutable' USING ERRCODE='23514';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.basket_group_id IS NOT NULL THEN
        UPDATE basket_hold_groups SET sealed=sealed WHERE id=NEW.basket_group_id
            RETURNING owner_id,created_at,expires_at,sealed INTO principal,started,expiry,closed;
        IF closed IS DISTINCT FROM false OR (NEW.owner_id,NEW.created_at,NEW.expires_at)
            IS DISTINCT FROM (principal,started,expiry) THEN
            RAISE EXCEPTION 'Hold requires its exact open basket identity' USING ERRCODE='23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_hold_basket_binding BEFORE INSERT OR UPDATE ON stock_holds FOR EACH ROW EXECUTE FUNCTION guard_hold_basket_binding();
CREATE TABLE basket_hold_members (
    hold_id text PRIMARY KEY REFERENCES stock_holds(id),
    group_id text NOT NULL REFERENCES basket_hold_groups(id),
    sku text NOT NULL,
    revision integer NOT NULL CHECK(revision>0),
    units integer NOT NULL CHECK(units BETWEEN 1 AND 1000),
    FOREIGN KEY(sku,revision) REFERENCES catalogue_boms(sku,revision),
    UNIQUE(group_id,sku)
);
CREATE INDEX basket_hold_members_group ON basket_hold_members(group_id,hold_id);
CREATE FUNCTION guard_basket_hold_member() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE principal text; started bigint; expiry bigint; closed boolean;
BEGIN
    IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Basket hold membership is immutable' USING ERRCODE='23514'; END IF;
    -- Advancing the row version prevents a stale repeatable-read import after sealing.
    UPDATE basket_hold_groups SET sealed=sealed WHERE id=NEW.group_id
        RETURNING owner_id,created_at,expires_at,sealed INTO principal,started,expiry,closed;
    IF closed IS DISTINCT FROM false OR NOT EXISTS(
        SELECT 1 FROM stock_holds h WHERE h.id=NEW.hold_id AND h.basket_group_id=NEW.group_id AND h.owner_id=principal
            AND h.sku=NEW.sku AND h.revision=NEW.revision AND h.units=NEW.units
            AND h.created_at=started AND h.expires_at=expiry AND h.state='active'
    ) THEN RAISE EXCEPTION 'Basket member requires exact active hold and open manifest' USING ERRCODE='23514'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_basket_hold_member BEFORE INSERT OR UPDATE OR DELETE ON basket_hold_members FOR EACH ROW EXECUTE FUNCTION guard_basket_hold_member();
CREATE FUNCTION guard_basket_hold_group() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Basket hold history is immutable' USING ERRCODE='23514'; END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.sealed THEN RAISE EXCEPTION 'Basket manifest must start open' USING ERRCODE='23514'; END IF;
        RETURN NEW;
    END IF;
    IF (NEW.id,NEW.owner_id,NEW.created_at,NEW.expires_at,NEW.members_count)
        IS DISTINCT FROM (OLD.id,OLD.owner_id,OLD.created_at,OLD.expires_at,OLD.members_count)
        OR (OLD.sealed AND NOT NEW.sealed) THEN
        RAISE EXCEPTION 'Basket hold identity is immutable' USING ERRCODE='23514';
    END IF;
    IF NEW.sealed AND NOT OLD.sealed AND (
        (SELECT count(*) FROM basket_hold_members WHERE group_id=NEW.id)<>NEW.members_count
        OR (SELECT count(*) FROM stock_holds WHERE basket_group_id=NEW.id)<>NEW.members_count
        OR EXISTS(SELECT 1 FROM basket_hold_members m JOIN stock_holds h ON h.id=m.hold_id WHERE m.group_id=NEW.id AND h.state<>'active')
    ) THEN RAISE EXCEPTION 'Cannot seal incomplete basket holds' USING ERRCODE='23514'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_basket_hold_group BEFORE INSERT OR UPDATE OR DELETE ON basket_hold_groups FOR EACH ROW EXECUTE FUNCTION guard_basket_hold_group();
CREATE FUNCTION require_complete_basket_holds() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM basket_hold_groups g WHERE g.id=NEW.id AND (
        NOT g.sealed OR (SELECT count(*) FROM basket_hold_members WHERE group_id=g.id)<>g.members_count
        OR (SELECT count(*) FROM stock_holds WHERE basket_group_id=g.id)<>g.members_count
    )) THEN RAISE EXCEPTION 'Incomplete basket manifest cannot commit' USING ERRCODE='23514'; END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_basket_holds AFTER INSERT OR UPDATE ON basket_hold_groups DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_basket_holds();
