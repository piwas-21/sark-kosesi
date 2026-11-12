-- Preserve legacy NULL receipts; an admitted attempt may be canceled only before
-- its durable start. Authentication and fresh clock authority remain in Rust.
ALTER TABLE checkout_order_cancellations
 ADD COLUMN attempt_id text REFERENCES payment_attempts(id);

CREATE OR REPLACE FUNCTION guard_checkout_cancellation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE attempt payment_attempts%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN
  RAISE EXCEPTION 'Cancellation history is immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Cancellation requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM guest_baskets WHERE id=NEW.basket_id FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Cancellation requires basket' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM checkout_orders o
  JOIN guest_quote_snapshots q ON q.id=o.quote_id AND q.basket_id=o.basket_id
  JOIN commerce_outbox e ON e.aggregate_id=o.id AND e.kind='checkout.order_created.v1'
   AND e.created_at=o.created_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',o.id)
  WHERE o.id=NEW.order_id AND o.basket_id=NEW.basket_id
   AND q.promotion_account_id IS NOT DISTINCT FROM NEW.promotion_account_id
   AND NEW.canceled_at>=o.created_at) THEN
  RAISE EXCEPTION 'Cancellation requires exact owned order and created event' USING ERRCODE='23514'; END IF;
 IF NEW.attempt_id IS NULL THEN
  IF EXISTS(SELECT 1 FROM payment_attempts WHERE order_id=NEW.order_id) THEN
   RAISE EXCEPTION 'Cancellation cannot follow a payment attempt' USING ERRCODE='23514'; END IF;
 ELSE
  SELECT * INTO attempt FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
  IF attempt.id IS NULL OR attempt.order_id IS DISTINCT FROM NEW.order_id
   OR attempt.basket_id IS DISTINCT FROM NEW.basket_id
   OR attempt.promotion_account_id IS DISTINCT FROM NEW.promotion_account_id
   OR NEW.canceled_at<attempt.created_at
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=attempt.order_id
    AND e.kind='payment.attempt_prepared.v1' AND e.created_at=attempt.created_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',attempt.order_id,'attempt_id',attempt.id)) THEN
   RAISE EXCEPTION 'Cancellation requires exact prepared attempt' USING ERRCODE='23514'; END IF;
  IF EXISTS(SELECT 1 FROM payment_dispatch_starts WHERE attempt_id=attempt.id) THEN
   RAISE EXCEPTION 'Cancellation cannot follow dispatch start' USING ERRCODE='23514'; END IF;
 END IF;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END;
$$;

-- A separate post-lock statement must see a winner committed during the wait.
-- Dispatch never takes the basket lock, so cancellation's basket -> attempt
-- order cannot invert against this native guard or the trusted dispatcher.
CREATE FUNCTION guard_dispatch_cancellation_exclusion() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE owner text;
BEGIN
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Dispatch requires read committed' USING ERRCODE='23514'; END IF;
 SELECT order_id INTO owner FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF owner IS NULL THEN RAISE EXCEPTION 'Dispatch requires an attempt' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM checkout_order_cancellations WHERE order_id=owner) THEN
  RAISE EXCEPTION 'Dispatch cannot follow cancellation' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_dispatch_cancellation_exclusion BEFORE INSERT ON payment_dispatch_starts
 FOR EACH ROW EXECUTE FUNCTION guard_dispatch_cancellation_exclusion();
