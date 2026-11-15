-- Private staff assertions only: no stock, carrier, payment or shipment action.
CREATE TABLE packing_custody (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 fence_key integer GENERATED ALWAYS AS IDENTITY UNIQUE CHECK(fence_key>0),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 allocation_kind text NOT NULL CHECK(allocation_kind IN ('original','late')),
 allocation_id text NOT NULL CHECK(allocation_id ~ '^[a-f0-9]{32}$'),
 actor_id text NOT NULL REFERENCES staff_accounts(id),
 effect_key text NOT NULL UNIQUE CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 generation smallint NOT NULL CHECK(generation BETWEEN 1 AND 64),
 action text NOT NULL CHECK(action IN ('packed','released')),
 target_id text REFERENCES packing_custody(id),
 units jsonb NOT NULL CHECK(jsonb_typeof(units)='array' AND octet_length(units::text)<=4096),
 packages jsonb NOT NULL CHECK(jsonb_typeof(packages)='array' AND octet_length(packages::text)<=65536),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 event_id text NOT NULL UNIQUE CHECK(event_id ~ '^[a-f0-9]{32}$'),
 transaction_id xid8 NOT NULL,
 UNIQUE(order_id,generation),
 CHECK((action='packed' AND target_id IS NULL) OR (action='released' AND target_id IS NOT NULL AND units='[]' AND packages='[]'))
);
CREATE UNIQUE INDEX packing_custody_one_release ON packing_custody(target_id) WHERE action='released';
CREATE FUNCTION packing_custody_digest(r packing_custody) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',r.id,'order_id',r.order_id,'attempt_id',r.attempt_id,
 'recognition_id',r.recognition_id,'allocation_kind',r.allocation_kind,'allocation_id',r.allocation_id,
 'actor_id',r.actor_id,'effect_key',r.effect_key,'generation',r.generation,'action',r.action,
 'target_id',r.target_id,'units',r.units,'packages',r.packages,'recorded_at',r.recorded_at,'event_id',r.event_id)::text,'UTF8')),'hex')
$$;
CREATE FUNCTION packing_custody_event(r packing_custody) RETURNS jsonb LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT jsonb_build_object('schema_version',1,'order_id',r.order_id,'record_id',r.id,'record_sha256',packing_custody_digest(r))
$$;

-- Validate the entire bounded journal. Package values are assertions, not evidence
-- of physical possession. Numeric checks precede every cast.
CREATE FUNCTION packing_custody_history_exact(wanted text) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE r packing_custody%ROWTYPE; previous packing_custody%ROWTYPE; item jsonb; pkg jsonb; part jsonb;
 n integer:=0; last_at bigint:=0; cnt integer; event_count integer; original_attempt text;
BEGIN
 IF NOT EXISTS(SELECT 1 FROM checkout_orders WHERE id=wanted AND octet_length(snapshot::text)<=65536
  AND jsonb_typeof(snapshot->'quote'->'lines')='array' AND jsonb_array_length(snapshot->'quote'->'lines') BETWEEN 1 AND 20) THEN RETURN false; END IF;
 FOR r IN SELECT * FROM packing_custody WHERE order_id=wanted ORDER BY generation LIMIT 65 LOOP
  n=n+1;
  IF n=1 THEN original_attempt=r.attempt_id; END IF;
  IF n>64 OR r.generation<>n OR r.recorded_at<last_at OR r.attempt_id IS DISTINCT FROM original_attempt
   OR r.transaction_id IS NULL OR NOT EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=r.order_id
       AND a.attempt_id=r.attempt_id AND a.kind=r.allocation_kind AND a.receipt_id=r.allocation_id)
   OR NOT late_stock_received(r.order_id,r.attempt_id,r.recognition_id,r.recorded_at,false)
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=r.event_id AND e.aggregate_id=r.order_id
       AND e.kind='order.packing_custody_recorded.v1' AND e.created_at=r.recorded_at AND e.payload=packing_custody_event(r)) THEN RETURN false; END IF;
  IF r.action='packed' THEN
   IF r.target_id IS NOT NULL OR jsonb_array_length(r.units) NOT BETWEEN 1 AND 20 OR jsonb_array_length(r.packages) NOT BETWEEN 1 AND 32 THEN RETURN false; END IF;
   FOR item IN SELECT value FROM jsonb_array_elements(r.units) LOOP
    IF jsonb_typeof(item)<>'object' OR item IS DISTINCT FROM jsonb_build_object('sku',item->>'sku','units',item->'units')
     OR item->>'sku' IS NULL OR jsonb_typeof(item->'units') IS DISTINCT FROM 'number' OR (item->>'units') !~ '^[1-9][0-9]{0,3}$'
     OR (item->>'units')::numeric>1000 OR NOT EXISTS(SELECT 1 FROM checkout_orders o, jsonb_array_elements(o.snapshot->'quote'->'lines') l WHERE o.id=wanted AND l->>'sku'=item->>'sku') THEN RETURN false; END IF;
   END LOOP;
   SELECT count(*),count(DISTINCT value->>'sku') INTO cnt,event_count FROM jsonb_array_elements(r.units);
   IF cnt<>event_count THEN RETURN false; END IF;
   FOR pkg IN SELECT value FROM jsonb_array_elements(r.packages) LOOP
    IF jsonb_typeof(pkg)<>'array' OR jsonb_array_length(pkg) NOT BETWEEN 1 AND 64 THEN RETURN false; END IF;
    FOR part IN SELECT value FROM jsonb_array_elements(pkg) LOOP
     IF jsonb_typeof(part)<>'object' OR part IS DISTINCT FROM jsonb_build_object('code',part->>'code','quantity',part->'quantity')
      OR part->>'code' IS NULL OR jsonb_typeof(part->'quantity') IS DISTINCT FROM 'number' OR (part->>'quantity') !~ '^[1-9][0-9]{0,9}$' OR (part->>'quantity')::numeric>1000000000 THEN RETURN false; END IF;
    END LOOP;
    SELECT count(*),count(DISTINCT value->>'code') INTO cnt,event_count FROM jsonb_array_elements(pkg);
    IF cnt<>event_count THEN RETURN false; END IF;
   END LOOP;
   IF EXISTS(
    WITH expected AS (SELECT b.component_code AS code,sum((u->>'units')::numeric*b.quantity) AS qty
     FROM checkout_orders o, jsonb_array_elements(o.snapshot->'quote'->'lines') q,
       jsonb_array_elements(r.units) u JOIN catalogue_bom_lines b ON b.sku=u->>'sku'
     WHERE o.id=wanted AND q->>'sku'=u->>'sku' AND b.revision=(q->>'bom_revision')::integer GROUP BY b.component_code),
    actual AS (SELECT p->>'code' AS code,sum((p->>'quantity')::numeric) AS qty
     FROM jsonb_array_elements(r.packages) AS pk(value), jsonb_array_elements(pk.value) p GROUP BY p->>'code')
    SELECT 1 FROM expected FULL JOIN actual USING(code) WHERE expected.qty IS DISTINCT FROM actual.qty
   ) THEN RETURN false; END IF;
  ELSIF r.action='released' THEN
   SELECT * INTO previous FROM packing_custody WHERE id=r.target_id AND order_id=wanted AND action='packed';
   IF previous.id IS NULL OR previous.generation>=r.generation OR r.units<>'[]'::jsonb OR r.packages<>'[]'::jsonb THEN RETURN false; END IF;
  ELSE RETURN false; END IF;
  -- At every point, active claimed units cannot exceed the accepted quantity.
  IF EXISTS(SELECT 1 FROM checkout_orders o, jsonb_array_elements(o.snapshot->'quote'->'lines') q
    WHERE o.id=wanted AND (SELECT COALESCE(sum((u->>'units')::numeric),0) FROM packing_custody p,
      jsonb_array_elements(p.units) u WHERE p.order_id=wanted AND p.action='packed' AND p.generation<=r.generation
      AND u->>'sku'=q->>'sku' AND NOT EXISTS(SELECT 1 FROM packing_custody rel WHERE rel.target_id=p.id AND rel.generation<=r.generation)) > (q->>'quantity')::numeric) THEN RETURN false; END IF;
  -- Preserve one remaining journal slot for each active assertion so every
  -- accepted pack can later be released without deleting history.
  SELECT count(*) INTO cnt FROM packing_custody p WHERE p.order_id=wanted AND p.action='packed'
   AND p.generation<=r.generation AND NOT EXISTS(SELECT 1 FROM packing_custody rel
    WHERE rel.target_id=p.id AND rel.action='released' AND rel.generation<=r.generation);
  IF n+cnt>64 THEN RETURN false; END IF;
  last_at=r.recorded_at;
 END LOOP;
 SELECT count(*) INTO event_count FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=wanted AND kind='order.packing_custody_recorded.v1' LIMIT 65) e;
 RETURN event_count=n;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;
CREATE FUNCTION guard_packing_custody() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE previous packing_custody%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Packing custody is immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Packing requires read committed' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT * INTO previous FROM packing_custody WHERE order_id=NEW.order_id ORDER BY generation DESC LIMIT 1;
 IF (previous.id IS NULL AND NEW.generation<>1) OR (previous.id IS NOT NULL AND
  (NEW.generation<>previous.generation+1 OR NEW.recorded_at<previous.recorded_at OR previous.transaction_id=pg_current_xact_id())) THEN
  RAISE EXCEPTION 'Packing requires contiguous separate transactions' USING ERRCODE='23514'; END IF;
 IF NEW.action='packed' AND NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.recorded_at,true) THEN
  RAISE EXCEPTION 'Packing requires current recognition' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_packing_custody BEFORE INSERT OR UPDATE OR DELETE ON packing_custody FOR EACH ROW EXECUTE FUNCTION guard_packing_custody();
CREATE FUNCTION require_complete_packing_custody() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR NOT packing_custody_history_exact(NEW.order_id)
  OR (NEW.action='packed' AND NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.recorded_at,true)) THEN
  RAISE EXCEPTION 'Packing requires exact history and reciprocal event' USING ERRCODE='23514'; END IF;
 PERFORM pg_advisory_xact_lock(1936876998, NEW.fence_key);
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_packing_custody AFTER INSERT ON packing_custody DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_packing_custody();

DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind)
 WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1',
  'order.refund_execution_proposed.v1','payment.paid_parent_totals_observed.v1','order.packing_custody_recorded.v1');
CREATE UNIQUE INDEX commerce_outbox_packing_custody_identity ON commerce_outbox((payload->>'record_id'))
 WHERE kind='order.packing_custody_recorded.v1';
-- Keep a completed current-source check valid through commit after early constraint evaluation.
CREATE FUNCTION guard_packing_custody_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE item record; r packing_custody%ROWTYPE; attempts text[]:='{}'; orders text[]:='{}';
BEGIN
 IF TG_TABLE_NAME='payment_recognition_current' THEN
  IF TG_OP<>'DELETE' THEN orders=array_append(orders,NEW.order_id); END IF;
  IF TG_OP<>'INSERT' THEN orders=array_append(orders,OLD.order_id); END IF;
 ELSE
  IF TG_OP<>'DELETE' THEN attempts=array_append(attempts,NEW.attempt_id); END IF;
  IF TG_OP<>'INSERT' THEN attempts=array_append(attempts,OLD.attempt_id); END IF;
 END IF;
 FOR item IN SELECT objid::bigint AS key FROM pg_locks WHERE locktype='advisory'
  AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND pid=pg_backend_pid()
  AND mode='ExclusiveLock' AND granted AND classid=1936876998::oid AND objsubid=2 LOOP
  SELECT * INTO r FROM packing_custody WHERE fence_key=item.key;
  IF r.id IS NOT NULL AND (r.attempt_id=ANY(attempts) OR r.order_id=ANY(orders)) THEN
   RAISE EXCEPTION 'Completed packing source cannot change in this transaction' USING ERRCODE='23514'; END IF;
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END;
$$;
CREATE TRIGGER aa_packing_custody_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_observations FOR EACH ROW EXECUTE FUNCTION guard_packing_custody_seal();
CREATE TRIGGER aa_packing_custody_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_identity_resolutions FOR EACH ROW EXECUTE FUNCTION guard_packing_custody_seal();
CREATE TRIGGER aa_packing_custody_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognitions FOR EACH ROW EXECUTE FUNCTION guard_packing_custody_seal();
CREATE TRIGGER aa_packing_custody_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognition_current FOR EACH ROW EXECUTE FUNCTION guard_packing_custody_seal();

ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN
 ('checkout.order_created.v1','checkout.order_canceled.v1','payment.attempt_prepared.v1',
  'payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1',
  'order.stock_late_allocated.v1','order.stock_recovery_review.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1',
  'order.refund_execution_proposed.v1','payment.paid_parent_totals_observed.v1','order.packing_custody_recorded.v1'));

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
 ELSIF NEW.kind='order.packing_custody_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM packing_custody p WHERE p.event_id=NEW.id AND p.order_id=NEW.aggregate_id
   AND p.recorded_at=NEW.created_at AND p.transaction_id=pg_current_xact_id()
   AND NEW.payload=packing_custody_event(p)) THEN
   RAISE EXCEPTION 'Packing event requires same-transaction journal' USING ERRCODE='23514'; END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
