-- Private authenticated discovery evidence only; no payment, financial or stock transition.
-- Match the Rust provider-ID validator, including Unicode whitespace and controls.
CREATE FUNCTION payment_reversal_page_provider_id(value text, prefix text)
 RETURNS boolean LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT left(value,length(prefix))=prefix AND octet_length(value)>octet_length(prefix)
  AND octet_length(value)<=256
  AND value !~ U&'[\0001-\001F\007F-\009F]'
  AND value !~ U&'^[\0009-\000D\0020\0085\00A0\1680\2000-\200A\2028\2029\202F\205F\3000]'
  AND value !~ U&'[\0009-\000D\0020\0085\00A0\1680\2000-\200A\2028\2029\202F\205F\3000]$'
$$;

CREATE TABLE payment_reversal_pages (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 payment_id text NOT NULL CHECK(payment_reversal_page_provider_id(payment_id,'tr_')),
 reversal_kind text NOT NULL CHECK(reversal_kind IN ('refund','chargeback')),
 credential_fingerprint text NOT NULL CHECK(credential_fingerprint ~ '^[a-f0-9]{64}$'),
 scan_id text NOT NULL CHECK(scan_id ~ '^[a-f0-9]{32}$'),
 page_number integer NOT NULL CHECK(page_number BETWEEN 1 AND 32),
 previous_page_id text UNIQUE REFERENCES payment_reversal_pages(id),
 from_cursor text,
 next_cursor text,
 candidates text[] NOT NULL CHECK(cardinality(candidates)<=10 AND coalesce(array_ndims(candidates),1)=1
  AND (cardinality(candidates)=0 OR array_lower(candidates,1)=1)),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 event_id text NOT NULL UNIQUE CHECK(event_id ~ '^[a-f0-9]{32}$'),
 transaction_id xid8 NOT NULL DEFAULT pg_current_xact_id(),
 UNIQUE(attempt_id,effect_key), UNIQUE(scan_id,page_number),
 CHECK((page_number=1 AND scan_id=id AND previous_page_id IS NULL AND from_cursor IS NULL)
  OR (page_number>1 AND scan_id<>id AND previous_page_id IS NOT NULL AND from_cursor IS NOT NULL)),
 CHECK(from_cursor IS NULL OR payment_reversal_page_provider_id(from_cursor,CASE reversal_kind WHEN 'refund' THEN 're_' ELSE 'chb_' END)),
 CHECK(next_cursor IS NULL OR payment_reversal_page_provider_id(next_cursor,CASE reversal_kind WHEN 'refund' THEN 're_' ELSE 'chb_' END)),
 CHECK(from_cursor IS NULL OR next_cursor IS NULL OR from_cursor<>next_cursor)
);

CREATE FUNCTION payment_reversal_page_event(p payment_reversal_pages)
 RETURNS jsonb LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT jsonb_build_object('schema_version',1,'order_id',p.order_id,'attempt_id',p.attempt_id,
  'page_id',p.id,'page_sha256',encode(sha256(convert_to(jsonb_build_array(
   p.id,p.order_id,p.attempt_id,p.effect_key,p.payment_id,p.reversal_kind,p.credential_fingerprint,
   p.scan_id,p.page_number,p.previous_page_id,p.from_cursor,p.next_cursor,p.candidates,
   p.recorded_at,p.event_id,p.transaction_id::text
  )::text,'UTF8')),'hex'))
$$;

-- Serialize all writes to the owning immutable attempt; provenance is verified in Rust.
CREATE FUNCTION guard_payment_reversal_page() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE attempt payment_attempts%ROWTYPE; started bigint; history_count bigint; latest_at bigint;
 parent payment_reversal_pages%ROWTYPE; ancestor payment_reversal_pages%ROWTYPE;
 child payment_reversal_pages%ROWTYPE; traversed integer=0; prefix text;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Reversal pages are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Reversal page requires read committed' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 SELECT * INTO attempt FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT started_at INTO started FROM payment_dispatch_starts WHERE attempt_id=NEW.attempt_id;
 IF attempt.id IS NULL OR started IS NULL OR attempt.order_id IS DISTINCT FROM NEW.order_id
  OR NOT EXISTS(SELECT 1 FROM checkout_orders o WHERE o.id=NEW.order_id AND o.basket_id=attempt.basket_id)
  OR NEW.recorded_at<started
  OR attempt.snapshot->'binding'->>'id' IS DISTINCT FROM attempt.id
  OR attempt.snapshot->'binding'->>'order_id' IS DISTINCT FROM NEW.order_id
  OR attempt.snapshot->'binding'->>'basket_id' IS DISTINCT FROM attempt.basket_id
  OR attempt.snapshot->'binding'->>'currency' IS DISTINCT FROM 'EUR'
  OR attempt.snapshot->'binding'->'profile'->>'mode' IS DISTINCT FROM 'test'
  OR attempt.configuration->'profile' IS DISTINCT FROM attempt.snapshot->'binding'->'profile' THEN
  RAISE EXCEPTION 'Reversal page requires exact frozen order and dispatch binding' USING ERRCODE='23514'; END IF;
 SELECT count(*),max(recorded_at) INTO history_count,latest_at
 FROM (SELECT recorded_at FROM payment_reversal_pages WHERE attempt_id=NEW.attempt_id LIMIT 128) bounded;
 IF history_count>=128 THEN RAISE EXCEPTION 'Reversal page limit reached' USING ERRCODE='23514'; END IF;
 IF NEW.recorded_at<latest_at THEN RAISE EXCEPTION 'Reversal page recording time must not regress' USING ERRCODE='23514'; END IF;
 prefix=CASE NEW.reversal_kind WHEN 'refund' THEN 're_' ELSE 'chb_' END;
 IF EXISTS(SELECT 1 FROM unnest(NEW.candidates) candidate
  WHERE candidate IS NULL OR NOT payment_reversal_page_provider_id(candidate,prefix))
  OR (SELECT count(DISTINCT candidate) FROM unnest(NEW.candidates) candidate)<>cardinality(NEW.candidates) THEN
  RAISE EXCEPTION 'Invalid reversal page candidates' USING ERRCODE='23514'; END IF;
 IF NEW.previous_page_id IS NOT NULL THEN
  SELECT * INTO parent FROM payment_reversal_pages WHERE id=NEW.previous_page_id;
  IF parent.id IS NULL OR parent.order_id IS DISTINCT FROM NEW.order_id
   OR parent.attempt_id IS DISTINCT FROM NEW.attempt_id OR parent.payment_id IS DISTINCT FROM NEW.payment_id
   OR parent.reversal_kind IS DISTINCT FROM NEW.reversal_kind
   OR parent.credential_fingerprint IS DISTINCT FROM NEW.credential_fingerprint
   OR parent.scan_id IS DISTINCT FROM NEW.scan_id OR parent.page_number+1<>NEW.page_number
   OR parent.next_cursor IS NULL OR parent.next_cursor IS DISTINCT FROM NEW.from_cursor
   OR NEW.recorded_at<parent.recorded_at THEN
   RAISE EXCEPTION 'Reversal page requires exact predecessor' USING ERRCODE='23514'; END IF;
  ancestor=parent;
  LOOP
   traversed=traversed+1;
   IF traversed>32 THEN RAISE EXCEPTION 'Reversal page ancestry exceeds bound' USING ERRCODE='23514'; END IF;
   IF NEW.next_cursor IS NOT NULL AND NEW.next_cursor=ancestor.from_cursor THEN
    RAISE EXCEPTION 'Cyclic reversal page cursor' USING ERRCODE='23514'; END IF;
   EXIT WHEN ancestor.previous_page_id IS NULL;
   child=ancestor;
   SELECT * INTO ancestor FROM payment_reversal_pages WHERE id=child.previous_page_id;
   IF ancestor.id IS NULL OR ancestor.page_number+1<>child.page_number
    OR ancestor.order_id IS DISTINCT FROM NEW.order_id OR ancestor.attempt_id IS DISTINCT FROM NEW.attempt_id
    OR ancestor.payment_id IS DISTINCT FROM NEW.payment_id OR ancestor.reversal_kind IS DISTINCT FROM NEW.reversal_kind
    OR ancestor.credential_fingerprint IS DISTINCT FROM NEW.credential_fingerprint
    OR ancestor.scan_id IS DISTINCT FROM NEW.scan_id OR ancestor.next_cursor IS DISTINCT FROM child.from_cursor
    OR child.recorded_at<ancestor.recorded_at THEN
    RAISE EXCEPTION 'Invalid reversal page ancestry' USING ERRCODE='23514'; END IF;
  END LOOP;
  IF ancestor.id IS DISTINCT FROM NEW.scan_id OR ancestor.page_number<>1 THEN
   RAISE EXCEPTION 'Invalid reversal scan root' USING ERRCODE='23514'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_reversal_page BEFORE INSERT OR UPDATE OR DELETE ON payment_reversal_pages
 FOR EACH ROW EXECUTE FUNCTION guard_payment_reversal_page();

CREATE FUNCTION require_complete_reversal_page() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR NOT EXISTS(
  SELECT 1 FROM commerce_outbox e WHERE e.id=NEW.event_id AND e.aggregate_id=NEW.order_id
   AND e.kind='payment.reversal_page_recorded.v1' AND e.created_at=NEW.recorded_at
   AND e.payload=payment_reversal_page_event(NEW)) THEN
  RAISE EXCEPTION 'Reversal page requires exact same-transaction event' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_reversal_page AFTER INSERT ON payment_reversal_pages
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_reversal_page();

DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind)
 WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1');
CREATE UNIQUE INDEX commerce_outbox_reversal_page_identity ON commerce_outbox((payload->>'page_id'))
 WHERE kind='payment.reversal_page_recorded.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN
 ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1',
  'payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1',
  'payment.recognized.v1','order.stock_settled.v1','order.stock_late_allocated.v1',
  'order.stock_recovery_review.v1','order.stock_recovery_decided.v1','order.refund_reserved.v1',
  'payment.reversal_observed.v1','payment.reversal_page_recorded.v1'));

CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE; settlement order_stock_settlements%ROWTYPE;
 allocation late_stock_allocations%ROWTYPE; review late_stock_review_cases%ROWTYPE; decision late_stock_dispositions%ROWTYPE;
 reservation refund_reservations%ROWTYPE; reversal payment_reversal_observations%ROWTYPE;
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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
