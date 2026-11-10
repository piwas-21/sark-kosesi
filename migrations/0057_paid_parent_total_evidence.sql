-- Private paid-parent aggregate evidence. Neither operation nor receipt grants
-- refund, payment, stock, or provider-dispatch authority.
CREATE TABLE paid_parent_total_operations (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 through_observation bigint NOT NULL CHECK(through_observation BETWEEN 0 AND 1000),
 profile_id text NOT NULL CHECK(payment_reversal_page_provider_id(profile_id,'pfl_')),
 payment_id text NOT NULL CHECK(payment_reversal_page_provider_id(payment_id,'tr_')),
 credential_fingerprint text NOT NULL CHECK(credential_fingerprint ~ '^[a-f0-9]{64}$'),
 started_at bigint NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740931),
 fetch_before bigint NOT NULL CONSTRAINT paid_parent_total_operations_fetch_before_check
  CHECK(fetch_before=started_at+60),
 transaction_id xid8 NOT NULL,
 UNIQUE(attempt_id,effect_key)
);
CREATE INDEX paid_parent_total_operation_transaction ON paid_parent_total_operations(attempt_id,transaction_id);
CREATE INDEX paid_parent_total_operation_order_transaction ON paid_parent_total_operations(order_id,transaction_id);

CREATE TABLE paid_parent_total_receipts (
 operation_id text PRIMARY KEY REFERENCES paid_parent_total_operations(id),
 refunded_minor bigint CHECK(refunded_minor BETWEEN 0 AND 9007199254740991),
 remaining_minor bigint CHECK(remaining_minor BETWEEN 0 AND 9007199254740991),
 captured_minor bigint CHECK(captured_minor BETWEEN 0 AND 9007199254740991),
 charged_back_minor bigint CHECK(charged_back_minor BETWEEN 0 AND 9007199254740991),
 fetched_at bigint NOT NULL CHECK(fetched_at BETWEEN 0 AND 9007199254740991),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 event_id text UNIQUE NOT NULL CHECK(event_id ~ '^[a-f0-9]{32}$'),
 transaction_id xid8 NOT NULL
);

-- Bounded structural checks are independent of the more expensive recognition
-- proof. They remain available to historical readers after admission.
CREATE FUNCTION paid_parent_total_operation_shape(o paid_parent_total_operations) RETURNS boolean
 LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
 IF o.id IS NULL OR o.id !~ '^[a-f0-9]{32}$'
  OR o.order_id IS NULL OR o.order_id !~ '^[a-f0-9]{32}$'
  OR o.attempt_id IS NULL OR o.attempt_id !~ '^[a-f0-9]{32}$'
  OR o.recognition_id IS NULL OR o.recognition_id !~ '^[a-f0-9]{32}$'
  OR o.effect_key IS NULL OR o.effect_key !~ '^[a-zA-Z0-9-]{1,128}$'
  OR o.through_observation IS NULL OR o.through_observation NOT BETWEEN 0 AND 1000
  OR o.transaction_id IS NULL OR o.started_at IS NULL OR o.started_at NOT BETWEEN 0 AND 9007199254740931
  OR NOT coalesce(payment_reversal_page_provider_id(o.profile_id,'pfl_'),false)
  OR NOT coalesce(payment_reversal_page_provider_id(o.payment_id,'tr_'),false)
  OR o.credential_fingerprint IS NULL OR o.credential_fingerprint !~ '^[a-f0-9]{64}$' THEN RETURN false; END IF;
 RETURN o.fetch_before IS NOT DISTINCT FROM o.started_at+60;
END;
$$;

CREATE FUNCTION paid_parent_total_receipt_shape(r paid_parent_total_receipts, o paid_parent_total_operations) RETURNS boolean
 LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
 IF NOT paid_parent_total_operation_shape(o)
  OR r.operation_id IS DISTINCT FROM o.id
  OR r.transaction_id IS NULL OR r.transaction_id IS NOT DISTINCT FROM o.transaction_id
  OR r.event_id IS NULL OR r.event_id !~ '^[a-f0-9]{32}$'
  OR r.fetched_at IS NULL OR r.fetched_at NOT BETWEEN 0 AND 9007199254740991
  OR r.recorded_at IS NULL OR r.recorded_at NOT BETWEEN 0 AND 9007199254740991
  OR r.fetched_at<o.started_at OR r.recorded_at<r.fetched_at
  OR (r.refunded_minor IS NOT NULL AND r.refunded_minor NOT BETWEEN 0 AND 9007199254740991)
  OR (r.remaining_minor IS NOT NULL AND r.remaining_minor NOT BETWEEN 0 AND 9007199254740991)
  OR (r.captured_minor IS NOT NULL AND r.captured_minor NOT BETWEEN 0 AND 9007199254740991)
  OR (r.charged_back_minor IS NOT NULL AND r.charged_back_minor NOT BETWEEN 0 AND 9007199254740991)
 THEN RETURN false; END IF;
 RETURN true;
END;
$$;

-- Exact historic recognition, source and identity; current pointer is required
-- only when admitting a new operation, not when retaining later evidence.
CREATE FUNCTION paid_parent_total_operation_source(o paid_parent_total_operations, require_current boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE r payment_recognitions%ROWTYPE; resolution payment_identity_resolutions%ROWTYPE;
 attempt payment_attempts%ROWTYPE;
BEGIN
 IF NOT paid_parent_total_operation_shape(o) THEN RETURN false; END IF;
 SELECT * INTO r FROM payment_recognitions WHERE id=o.recognition_id;
 SELECT * INTO resolution FROM payment_identity_resolutions WHERE id=r.resolution_id;
 SELECT * INTO attempt FROM payment_attempts WHERE id=o.attempt_id;
 IF r.id IS NULL OR resolution.id IS NULL OR attempt.id IS NULL
  OR r.order_id IS DISTINCT FROM o.order_id OR r.attempt_id IS DISTINCT FROM o.attempt_id
  OR r.state IS DISTINCT FROM 'received' OR r.through_observation IS DISTINCT FROM o.through_observation
  OR o.profile_id IS DISTINCT FROM attempt.configuration->'profile'->>'id'
  OR jsonb_typeof(resolution.summary->'provider_ids') IS DISTINCT FROM 'array'
  OR jsonb_typeof(resolution.summary->'paid_provider_ids') IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 IF jsonb_array_length(resolution.summary->'provider_ids')<>1
  OR jsonb_array_length(resolution.summary->'paid_provider_ids')<>1
  OR o.payment_id IS DISTINCT FROM resolution.summary->'provider_ids'->>0
  OR o.payment_id IS DISTINCT FROM resolution.summary->'paid_provider_ids'->>0
  OR NOT EXISTS(SELECT 1 FROM payment_provider_identity_claims c
   WHERE c.profile_id=o.profile_id AND c.provider_id=o.payment_id AND c.attempt_id=o.attempt_id)
  OR NOT late_stock_received(o.order_id,o.attempt_id,o.recognition_id,o.started_at,require_current)
 THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION guard_paid_parent_total_operation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE history_count bigint; latest_at bigint;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Paid parent operations are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Paid parent operation requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Paid parent operation requires attempt' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 SELECT count(*),max(started_at) INTO history_count,latest_at
  FROM (SELECT started_at FROM paid_parent_total_operations WHERE attempt_id=NEW.attempt_id LIMIT 128) bounded;
 IF history_count>=128 THEN RAISE EXCEPTION 'Paid parent operation limit reached' USING ERRCODE='23514'; END IF;
 IF NEW.started_at<latest_at THEN RAISE EXCEPTION 'Paid parent operation time must not regress' USING ERRCODE='23514'; END IF;
 IF NOT paid_parent_total_operation_source(NEW,true) THEN
  RAISE EXCEPTION 'Paid parent operation requires exact current received source' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_paid_parent_total_operation BEFORE INSERT OR UPDATE OR DELETE ON paid_parent_total_operations
 FOR EACH ROW EXECUTE FUNCTION guard_paid_parent_total_operation();

CREATE FUNCTION require_paid_parent_total_operation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
  OR NOT paid_parent_total_operation_source(NEW,true) THEN
  RAISE EXCEPTION 'Paid parent operation requires exact current received source' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_paid_parent_total_operation AFTER INSERT ON paid_parent_total_operations
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_paid_parent_total_operation();

-- Do not allow same-transaction source changes after an operation was admitted,
-- even if a caller has forced the deferred source check early.
CREATE FUNCTION guard_paid_parent_total_operation_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE attempts text[]:='{}'; orders text[]:='{}';
BEGIN
 IF TG_TABLE_NAME='payment_recognition_current' THEN
  IF TG_OP<>'DELETE' THEN orders=array_append(orders,NEW.order_id); END IF;
  IF TG_OP<>'INSERT' THEN orders=array_append(orders,OLD.order_id); END IF;
 ELSE
  IF TG_OP<>'DELETE' THEN attempts=array_append(attempts,NEW.attempt_id); END IF;
  IF TG_OP<>'INSERT' THEN attempts=array_append(attempts,OLD.attempt_id); END IF;
 END IF;
 IF EXISTS(SELECT 1 FROM paid_parent_total_operations o
  WHERE o.attempt_id=ANY(attempts) AND o.transaction_id=pg_current_xact_id())
  OR EXISTS(SELECT 1 FROM paid_parent_total_operations o
   WHERE o.order_id=ANY(orders) AND o.transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Paid parent operation source is sealed for this transaction' USING ERRCODE='23514'; END IF;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_paid_parent_total_operation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_observations FOR EACH ROW EXECUTE FUNCTION guard_paid_parent_total_operation_seal();
CREATE TRIGGER aa_paid_parent_total_operation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_identity_resolutions FOR EACH ROW EXECUTE FUNCTION guard_paid_parent_total_operation_seal();
CREATE TRIGGER aa_paid_parent_total_operation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognitions FOR EACH ROW EXECUTE FUNCTION guard_paid_parent_total_operation_seal();
CREATE TRIGGER aa_paid_parent_total_operation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognition_current FOR EACH ROW EXECUTE FUNCTION guard_paid_parent_total_operation_seal();

CREATE FUNCTION paid_parent_total_receipt_digest(r paid_parent_total_receipts, o paid_parent_total_operations) RETURNS text
 LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_array(o.id,o.order_id,o.attempt_id,o.effect_key,
  o.recognition_id,o.through_observation,o.profile_id,o.payment_id,o.credential_fingerprint,
  o.started_at,o.fetch_before,o.transaction_id::text,r.operation_id,r.refunded_minor,r.remaining_minor,
  r.captured_minor,r.charged_back_minor,r.fetched_at,r.recorded_at,r.event_id,r.transaction_id::text)::text,'UTF8')),'hex')
$$;
CREATE FUNCTION paid_parent_total_receipt_event(r paid_parent_total_receipts, o paid_parent_total_operations) RETURNS jsonb
 LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT jsonb_build_object('schema_version',1,'order_id',o.order_id,'attempt_id',o.attempt_id,
  'operation_id',o.id,'receipt_sha256',paid_parent_total_receipt_digest(r,o))
$$;

CREATE FUNCTION guard_paid_parent_total_receipt() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE o paid_parent_total_operations%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Paid parent receipts are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Paid parent receipt requires read committed' USING ERRCODE='23514'; END IF;
 SELECT * INTO o FROM paid_parent_total_operations WHERE id=NEW.operation_id;
 IF o.id IS NULL THEN RAISE EXCEPTION 'Paid parent receipt requires operation' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=o.attempt_id FOR UPDATE;
 NEW.transaction_id=pg_current_xact_id();
 IF NOT paid_parent_total_receipt_shape(NEW,o)
  OR NOT paid_parent_total_operation_source(o,false) THEN
  RAISE EXCEPTION 'Paid parent receipt requires exact historical source and chronology' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_paid_parent_total_receipt BEFORE INSERT OR UPDATE OR DELETE ON paid_parent_total_receipts
 FOR EACH ROW EXECUTE FUNCTION guard_paid_parent_total_receipt();

CREATE FUNCTION require_paid_parent_total_receipt() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE o paid_parent_total_operations%ROWTYPE;
BEGIN
 SELECT * INTO o FROM paid_parent_total_operations WHERE id=NEW.operation_id;
 IF o.id IS NULL OR NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
  OR NOT paid_parent_total_receipt_shape(NEW,o)
  OR NOT paid_parent_total_operation_source(o,false)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=NEW.event_id AND e.aggregate_id=o.order_id
   AND e.kind='payment.paid_parent_totals_observed.v1' AND e.created_at=NEW.recorded_at
   AND e.payload=paid_parent_total_receipt_event(NEW,o)) THEN
  RAISE EXCEPTION 'Paid parent receipt requires exact reciprocal event' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_paid_parent_total_receipt AFTER INSERT ON paid_parent_total_receipts
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_paid_parent_total_receipt();


-- Reuse all previous outbox guards and indexes; this event is repeated per
-- operation and gets its own identity uniqueness constraint.
DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind)
 WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1',
  'order.refund_execution_proposed.v1','payment.paid_parent_totals_observed.v1');
CREATE UNIQUE INDEX commerce_outbox_paid_parent_total_identity ON commerce_outbox((payload->>'operation_id'))
 WHERE kind='payment.paid_parent_totals_observed.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN
 ('checkout.order_created.v1','checkout.order_canceled.v1','payment.attempt_prepared.v1',
  'payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1',
  'order.stock_late_allocated.v1','order.stock_recovery_review.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1',
  'order.refund_execution_proposed.v1','payment.paid_parent_totals_observed.v1'));

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
 ELSIF NEW.kind='payment.paid_parent_totals_observed.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Paid parent event requires read committed' USING ERRCODE='23514'; END IF;
  IF NOT EXISTS(SELECT 1 FROM paid_parent_total_receipts r
   JOIN paid_parent_total_operations o ON o.id=r.operation_id
   WHERE r.event_id=NEW.id AND r.transaction_id=pg_current_xact_id()
    AND o.order_id=NEW.aggregate_id AND r.recorded_at=NEW.created_at
    AND NEW.payload=paid_parent_total_receipt_event(r,o)) THEN
   RAISE EXCEPTION 'Paid parent event requires exact same-transaction receipt' USING ERRCODE='23514'; END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
