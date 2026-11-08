-- Private pre-attempt cancellation custody. This records no stock, promotion,
-- provider, or refund disposition. Customer authority remains in Rust.
CREATE TABLE checkout_order_cancellations (
 order_id text PRIMARY KEY REFERENCES checkout_orders(id),
 basket_id text NOT NULL REFERENCES guest_baskets(id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 promotion_account_id bigint REFERENCES verified_customer_accounts(id),
 canceled_at bigint NOT NULL CHECK(canceled_at BETWEEN 0 AND 9007199254740991),
 event_id text UNIQUE NOT NULL CHECK(event_id ~ '^[a-f0-9]{32}$'),
 transaction_id xid8 NOT NULL,
 UNIQUE(basket_id,effect_key)
);

-- The basket is the common exclusive lock for payment admission and
-- cancellation. Separate statements after the lock obtain fresh READ COMMITTED
-- snapshots even when a concurrent writer committed while this one waited.
CREATE FUNCTION guard_checkout_cancellation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
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
 IF EXISTS(SELECT 1 FROM payment_attempts WHERE order_id=NEW.order_id) THEN
  RAISE EXCEPTION 'Cancellation cannot follow a payment attempt' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_checkout_cancellation BEFORE INSERT OR UPDATE OR DELETE ON checkout_order_cancellations
 FOR EACH ROW EXECUTE FUNCTION guard_checkout_cancellation();

CREATE FUNCTION guard_payment_cancellation_exclusion() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Payment admission requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM guest_baskets WHERE id=NEW.basket_id FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Payment admission requires basket' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM checkout_order_cancellations WHERE order_id=NEW.order_id) THEN
  RAISE EXCEPTION 'Payment admission cannot follow cancellation' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_payment_cancellation_exclusion BEFORE INSERT ON payment_attempts
 FOR EACH ROW EXECUTE FUNCTION guard_payment_cancellation_exclusion();

-- Keep every prior event kind and its existing validation path. The existing
-- uniqueness index also applies to the new one-per-order cancellation kind.
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN
 ('checkout.order_created.v1','checkout.order_canceled.v1','payment.attempt_prepared.v1',
  'payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1',
  'order.stock_late_allocated.v1','order.stock_recovery_review.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1',
  'order.refund_execution_proposed.v1'));

-- Retain the existing trigger and every previous event validation branch.
CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE; settlement order_stock_settlements%ROWTYPE;
 allocation late_stock_allocations%ROWTYPE; review late_stock_review_cases%ROWTYPE; decision late_stock_dispositions%ROWTYPE;
 reservation refund_reservations%ROWTYPE; proposal refund_execution_proposals%ROWTYPE; reversal payment_reversal_observations%ROWTYPE;
 page payment_reversal_pages%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Commerce events are immutable' USING ERRCODE='23514'; END IF;
 IF NEW.kind='checkout.order_created.v1' THEN
  IF NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',NEW.aggregate_id)
   OR NOT EXISTS(SELECT 1 FROM checkout_orders o WHERE o.id=NEW.aggregate_id AND o.created_at=NEW.created_at) THEN
   RAISE EXCEPTION 'Order event requires exact original binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.attempt_prepared.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_attempts a WHERE a.order_id=NEW.aggregate_id AND a.created_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id)) THEN
   RAISE EXCEPTION 'Prepared payment event requires exact attempt binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.reconciliation_required.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_dispatch_starts s JOIN payment_attempts a ON a.id=s.attempt_id WHERE a.order_id=NEW.aggregate_id AND s.started_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id)) THEN
   RAISE EXCEPTION 'Payment recovery event requires exact dispatch binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.observation_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_observations o JOIN payment_attempts a ON a.id=o.attempt_id WHERE a.order_id=NEW.aggregate_id AND o.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'observation_id',o.id)) THEN
   RAISE EXCEPTION 'Observation event requires exact historical fact binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.search_page_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_search_pages p JOIN payment_attempts a ON a.id=p.attempt_id WHERE a.order_id=NEW.aggregate_id AND p.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'page_id',p.id)) THEN
   RAISE EXCEPTION 'Search page event requires exact discovery binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.identity_resolved.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_identity_resolutions r JOIN payment_attempts a ON a.id=r.attempt_id WHERE a.order_id=NEW.aggregate_id AND r.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'resolution_id',r.id)) THEN
   RAISE EXCEPTION 'Resolution event requires exact evidence binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.recognized.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Recognition event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO recognition FROM payment_recognitions WHERE id=NEW.payload->>'recognition_id';
  PERFORM 1 FROM payment_attempts WHERE id=recognition.attempt_id FOR UPDATE;
  IF recognition.id IS NULL OR NOT payment_recognition_source(recognition,false)
   OR NEW.aggregate_id IS DISTINCT FROM recognition.order_id OR NEW.created_at IS DISTINCT FROM recognition.recorded_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',recognition.order_id,
     'attempt_id',recognition.attempt_id,'recognition_id',recognition.id,'recognition_sha256',payment_recognition_digest(recognition)) THEN
   RAISE EXCEPTION 'Recognition event requires exact immutable journal binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_settled.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Settlement event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO settlement FROM order_stock_settlements WHERE id=NEW.payload->>'settlement_id';
  PERFORM 1 FROM payment_attempts WHERE id=settlement.attempt_id FOR UPDATE;
  IF settlement.id IS NULL OR octet_length(settlement.footprint::text)>1048576 THEN
   RAISE EXCEPTION 'Settlement event requires bounded owning receipt' USING ERRCODE='23514'; END IF;
  IF settlement.id IS NULL OR NEW.aggregate_id IS DISTINCT FROM settlement.order_id
   OR NEW.created_at IS DISTINCT FROM settlement.settled_at
   OR NOT order_stock_effects_exact(settlement,false)
   OR NOT EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=settlement.order_id AND a.attempt_id=settlement.attempt_id AND a.kind='original' AND a.receipt_id=settlement.id)
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',settlement.order_id,
     'settlement_id',settlement.id,'settlement_sha256',order_stock_digest(settlement)) THEN
   RAISE EXCEPTION 'Settlement event requires exact immutable receipt and effects' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_late_allocated.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Late allocation event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO allocation FROM late_stock_allocations WHERE id=NEW.payload->>'allocation_id';
  PERFORM 1 FROM payment_attempts WHERE id=allocation.attempt_id FOR UPDATE;
  IF allocation.id IS NULL OR octet_length(allocation.footprint::text)>2097152 THEN
   RAISE EXCEPTION 'Late allocation event requires bounded owning receipt' USING ERRCODE='23514'; END IF;
  IF NOT late_stock_allocation_exact(allocation,false,false)
   OR NEW.aggregate_id IS DISTINCT FROM allocation.order_id OR NEW.created_at IS DISTINCT FROM allocation.allocated_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',allocation.order_id,
    'allocation_id',allocation.id,'allocation_sha256',late_stock_digest(allocation)) THEN
   RAISE EXCEPTION 'Late allocation event requires exact complete receipt and effects' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_recovery_review.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Stock review event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO review FROM late_stock_review_cases WHERE id=NEW.payload->>'case_id';
  PERFORM 1 FROM payment_attempts WHERE id=review.attempt_id FOR UPDATE;
  IF review.id IS NULL OR octet_length(review.footprint::text)>2097152 THEN
   RAISE EXCEPTION 'Stock review event requires bounded owning receipt' USING ERRCODE='23514'; END IF;
  IF NOT late_stock_review_exact(review,false)
   OR NEW.aggregate_id IS DISTINCT FROM review.order_id OR NEW.created_at IS DISTINCT FROM review.assessed_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',review.order_id,
    'case_id',review.id,'case_sha256',late_stock_review_digest(review)) THEN
   RAISE EXCEPTION 'Stock review event requires exact historical assessment' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_recovery_decided.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Stock recovery event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO decision FROM late_stock_dispositions WHERE id=NEW.payload->>'decision_id';
  PERFORM 1 FROM payment_attempts WHERE id=decision.attempt_id FOR UPDATE;
  IF decision.id IS NULL OR NEW.aggregate_id IS DISTINCT FROM decision.order_id OR NEW.created_at IS DISTINCT FROM decision.recorded_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',decision.order_id,
    'decision_id',decision.id,'decision_sha256',late_stock_disposition_digest(decision)) THEN
   RAISE EXCEPTION 'Stock recovery event requires exact decision binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.refund_reserved.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Refund event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO reservation FROM refund_reservations WHERE id=NEW.payload->>'reservation_id';
  PERFORM 1 FROM payment_attempts WHERE id=reservation.attempt_id FOR UPDATE;
  IF reservation.id IS NULL OR reservation.transaction_id IS DISTINCT FROM pg_current_xact_id()
   OR NEW.id IS DISTINCT FROM reservation.event_id OR NEW.aggregate_id IS DISTINCT FROM reservation.order_id
   OR NEW.created_at IS DISTINCT FROM reservation.reserved_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',reservation.order_id,
    'reservation_id',reservation.id,'reservation_sha256',refund_reservation_digest(reservation)) THEN
   RAISE EXCEPTION 'Refund event requires exact same-transaction reservation binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='payment.reversal_observed.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Reversal event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO reversal FROM payment_reversal_observations WHERE id=NEW.payload->>'observation_id';
  PERFORM 1 FROM payment_attempts WHERE id=reversal.attempt_id FOR UPDATE;
  IF reversal.id IS NULL OR reversal.transaction_id IS DISTINCT FROM pg_current_xact_id()
   OR NEW.id IS DISTINCT FROM reversal.event_id OR NEW.aggregate_id IS DISTINCT FROM reversal.order_id
   OR NEW.created_at IS DISTINCT FROM reversal.recorded_at
   OR NEW.payload IS DISTINCT FROM payment_reversal_observation_event(reversal) THEN
   RAISE EXCEPTION 'Reversal event requires exact same-transaction observation binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='payment.reversal_page_recorded.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Reversal page event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO page FROM payment_reversal_pages WHERE id=NEW.payload->>'page_id';
  PERFORM 1 FROM payment_attempts WHERE id=page.attempt_id FOR UPDATE;
  IF page.id IS NULL OR page.transaction_id IS DISTINCT FROM pg_current_xact_id()
   OR NEW.id IS DISTINCT FROM page.event_id OR NEW.aggregate_id IS DISTINCT FROM page.order_id
   OR NEW.created_at IS DISTINCT FROM page.recorded_at
   OR NEW.payload IS DISTINCT FROM payment_reversal_page_event(page) THEN
   RAISE EXCEPTION 'Reversal page event requires exact same-transaction page binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.refund_execution_proposed.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Refund proposal event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO proposal FROM refund_execution_proposals WHERE id=NEW.payload->>'proposal_id';
  PERFORM 1 FROM payment_attempts WHERE id=proposal.attempt_id FOR UPDATE;
  IF proposal.id IS NULL OR proposal.transaction_id IS DISTINCT FROM pg_current_xact_id()
   OR NEW.id IS DISTINCT FROM proposal.event_id OR NEW.aggregate_id IS DISTINCT FROM proposal.order_id
   OR NEW.created_at IS DISTINCT FROM proposal.proposed_at
   OR NEW.payload IS DISTINCT FROM refund_execution_proposal_event(proposal) THEN
   RAISE EXCEPTION 'Refund proposal event requires exact same-transaction binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='checkout.order_canceled.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM checkout_order_cancellations c
   WHERE c.order_id=NEW.aggregate_id AND c.event_id=NEW.id AND c.canceled_at=NEW.created_at
    AND c.transaction_id=pg_current_xact_id()
    AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',c.order_id)) THEN
   RAISE EXCEPTION 'Cancellation event requires exact same-transaction receipt' USING ERRCODE='23514'; END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;

CREATE FUNCTION require_complete_checkout_cancellation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=NEW.event_id
  AND e.aggregate_id=NEW.order_id AND e.kind='checkout.order_canceled.v1'
  AND e.created_at=NEW.canceled_at
  AND e.payload=jsonb_build_object('schema_version',1,'order_id',NEW.order_id)) THEN
  RAISE EXCEPTION 'Cancellation requires its exact event' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_checkout_cancellation AFTER INSERT ON checkout_order_cancellations
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_checkout_cancellation();
