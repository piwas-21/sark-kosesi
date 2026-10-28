-- Physical component balances only. No independent sellable-variant stock pools.
CREATE TABLE stock_balances (
    component_code text PRIMARY KEY REFERENCES physical_components(code),
    on_hand bigint NOT NULL DEFAULT 0 CHECK (on_hand >= 0),
    quarantine bigint NOT NULL DEFAULT 0 CHECK (quarantine >= 0),
    reserved bigint NOT NULL DEFAULT 0 CHECK (reserved >= 0),
    CHECK (quarantine + reserved <= on_hand)
);
CREATE TABLE stock_movements (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    recorded_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    effect_key text UNIQUE NOT NULL CHECK (length(btrim(effect_key)) BETWEEN 1 AND 128),
    component_code text NOT NULL REFERENCES physical_components(code),
    action text NOT NULL CHECK (action IN ('receive','quarantine','release-quarantine','dispose-quarantine','count')),
    quantity bigint NOT NULL CHECK (quantity >= 0 AND (quantity > 0 OR action = 'count')),
    actor text NOT NULL CHECK (length(btrim(actor)) BETWEEN 1 AND 128),
    reason text NOT NULL CHECK (length(btrim(reason)) BETWEEN 1 AND 500),
    on_hand_delta bigint NOT NULL,
    quarantine_delta bigint NOT NULL,
    reserved_delta bigint NOT NULL CHECK (reserved_delta = 0),
    on_hand_after bigint NOT NULL CHECK (on_hand_after >= 0),
    quarantine_after bigint NOT NULL CHECK (quarantine_after >= 0),
    reserved_after bigint NOT NULL CHECK (reserved_after >= 0),
    CHECK (quarantine_after + reserved_after <= on_hand_after)
);
CREATE INDEX stock_movements_component ON stock_movements(component_code,sequence);
CREATE FUNCTION guard_stock_history() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Stock history is append-only' USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER guard_stock_history BEFORE UPDATE OR DELETE ON stock_movements
FOR EACH ROW EXECUTE FUNCTION guard_stock_history();
