CREATE TABLE stock_holds (
    id text PRIMARY KEY CHECK (id ~ '^[0-9a-f]{32}$'),
    owner_id text NOT NULL CHECK (length(btrim(owner_id)) BETWEEN 1 AND 128),
    sku text NOT NULL,
    revision integer NOT NULL,
    units integer NOT NULL CHECK (units BETWEEN 1 AND 1000),
    created_at bigint NOT NULL CHECK (created_at >= 0),
    expires_at bigint NOT NULL CONSTRAINT stock_hold_window CHECK (expires_at > created_at AND expires_at-created_at <= 1800),
    state text NOT NULL DEFAULT 'building' CHECK (state IN ('building','active','released','consumed','expired')),
    FOREIGN KEY (sku,revision) REFERENCES catalogue_boms(sku,revision)
);
CREATE INDEX stock_holds_expiry ON stock_holds(expires_at,id) WHERE state='active';
CREATE TABLE stock_hold_lines (
    hold_id text NOT NULL REFERENCES stock_holds(id),
    component_code text NOT NULL REFERENCES physical_components(code),
    quantity bigint NOT NULL CHECK (quantity BETWEEN 1 AND 10000000),
    PRIMARY KEY (hold_id,component_code)
);

CREATE FUNCTION guard_hold_line() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE parent_state text;
BEGIN
    IF TG_OP <> 'INSERT' THEN
        RAISE EXCEPTION 'Hold snapshot is immutable' USING ERRCODE = '23514';
    END IF;
    -- Advance the row version too, so stale repeatable-read activation fails.
    UPDATE stock_holds SET state=state WHERE id=NEW.hold_id RETURNING state INTO parent_state;
    IF parent_state IS DISTINCT FROM 'building' THEN
        RAISE EXCEPTION 'Hold snapshot is sealed' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_hold_line BEFORE INSERT OR UPDATE OR DELETE ON stock_hold_lines
FOR EACH ROW EXECUTE FUNCTION guard_hold_line();

CREATE FUNCTION guard_hold_state() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'Hold history is immutable' USING ERRCODE = '23514';
    END IF;
    IF TG_OP = 'INSERT' THEN
        IF NEW.state <> 'building' THEN
            RAISE EXCEPTION 'Hold must start building' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF (NEW.id,NEW.owner_id,NEW.sku,NEW.revision,NEW.units,NEW.created_at,NEW.expires_at)
        IS DISTINCT FROM (OLD.id,OLD.owner_id,OLD.sku,OLD.revision,OLD.units,OLD.created_at,OLD.expires_at) THEN
        RAISE EXCEPTION 'Hold identity is immutable' USING ERRCODE = '23514';
    END IF;
    IF NEW.state = OLD.state THEN RETURN NEW; END IF;
    IF OLD.state='building' AND NEW.state='active' THEN
        IF NOT EXISTS (SELECT 1 FROM catalogue_boms WHERE sku=NEW.sku AND revision=NEW.revision AND sealed)
            OR NOT EXISTS (SELECT 1 FROM stock_hold_lines WHERE hold_id=NEW.id)
            OR EXISTS (
                (SELECT component_code,quantity::bigint*NEW.units FROM catalogue_bom_lines WHERE sku=NEW.sku AND revision=NEW.revision)
                EXCEPT (SELECT component_code,quantity FROM stock_hold_lines WHERE hold_id=NEW.id)
            ) OR EXISTS (
                (SELECT component_code,quantity FROM stock_hold_lines WHERE hold_id=NEW.id)
                EXCEPT (SELECT component_code,quantity::bigint*NEW.units FROM catalogue_bom_lines WHERE sku=NEW.sku AND revision=NEW.revision)
            ) THEN
            RAISE EXCEPTION 'Hold snapshot must match sealed BOM' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF OLD.state='active' AND NEW.state IN ('released','consumed','expired') THEN RETURN NEW; END IF;
    RAISE EXCEPTION 'Invalid hold transition' USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER guard_hold_state BEFORE INSERT OR UPDATE OR DELETE ON stock_holds
FOR EACH ROW EXECUTE FUNCTION guard_hold_state();

CREATE FUNCTION require_complete_hold() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM stock_holds WHERE id=NEW.id AND state='building') THEN
        RAISE EXCEPTION 'Cannot commit incomplete hold' USING ERRCODE = '23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_hold AFTER INSERT OR UPDATE ON stock_holds
DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_hold();

ALTER TABLE stock_movements ADD COLUMN hold_id text REFERENCES stock_holds(id);
CREATE UNIQUE INDEX stock_hold_effect ON stock_movements(hold_id,component_code,action) WHERE hold_id IS NOT NULL;
ALTER TABLE stock_movements ADD CONSTRAINT stock_movement_hold_reference CHECK (
    (action IN ('reserve','release-hold','consume-hold','expire-hold')) = (hold_id IS NOT NULL)
);
ALTER TABLE stock_movements DROP CONSTRAINT stock_movements_action_check;
ALTER TABLE stock_movements ADD CONSTRAINT stock_movements_action_check
CHECK (action IN ('receive','quarantine','release-quarantine','dispose-quarantine','count','reserve','release-hold','consume-hold','expire-hold'));
ALTER TABLE stock_movements DROP CONSTRAINT stock_movements_reserved_delta_check;
ALTER TABLE stock_movements ADD CONSTRAINT stock_movements_reserved_delta_check CHECK (
    (action IN ('receive','quarantine','release-quarantine','dispose-quarantine','count') AND reserved_delta=0)
    OR (action='reserve' AND reserved_delta=quantity AND on_hand_delta=0 AND quarantine_delta=0)
    OR (action IN ('release-hold','expire-hold') AND reserved_delta=-quantity AND on_hand_delta=0 AND quarantine_delta=0)
    OR (action='consume-hold' AND reserved_delta=-quantity AND on_hand_delta=-quantity AND quarantine_delta=0)
);
