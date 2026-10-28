-- Operational retry state, separate from immutable component movement history.
CREATE TABLE stock_hold_expiry_failures (
    hold_id text PRIMARY KEY REFERENCES stock_holds(id),
    attempts integer NOT NULL CHECK (attempts BETWEEN 1 AND 1000000),
    last_attempt_at bigint NOT NULL CHECK (last_attempt_at >= 0),
    next_attempt_at bigint NOT NULL CHECK (next_attempt_at > last_attempt_at),
    code text NOT NULL CHECK (code IN ('invalid_stock','invalid_hold','database_error')),
    resolved_at bigint CHECK (resolved_at >= 0)
);
