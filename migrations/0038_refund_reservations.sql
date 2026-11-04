-- Private reservations of captured value. No provider refund, credit or stock effect.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CHECK(capability IN
 ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review','promotion.manage','promotion.review','payment.reconcile','stock.review','stock.recover','refund.prepare'));

CREATE TABLE refund_reservations (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 fence_key integer GENERATED ALWAYS AS IDENTITY UNIQUE CHECK(fence_key>0),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 actor_id text NOT NULL REFERENCES staff_accounts(id),
 effect_key text NOT NULL UNIQUE CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 generation smallint NOT NULL CHECK(generation BETWEEN 1 AND 16),
 reason text NOT NULL CHECK(octet_length(reason)<=240 AND btrim(reason)=reason AND reason<>'' AND reason !~ '[[:cntrl:]]'),
 reserved_at bigint NOT NULL CHECK(reserved_at BETWEEN 0 AND 9007199254740991),
 allocation jsonb NOT NULL CHECK(jsonb_typeof(allocation)='object' AND octet_length(allocation::text)<=16384),
 event_id text NOT NULL UNIQUE CHECK(event_id ~ '^[a-f0-9]{32}$'),
 transaction_id xid8 NOT NULL,
 UNIQUE(order_id,generation)
);
CREATE FUNCTION refund_reservation_digest(r refund_reservations) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',r.id,'order_id',r.order_id,
  'attempt_id',r.attempt_id,'recognition_id',r.recognition_id,'actor_id',r.actor_id,
  'effect_key',r.effect_key,'generation',r.generation,'reason',r.reason,
  'reserved_at',r.reserved_at,'allocation',r.allocation,'event_id',r.event_id)::text,'UTF8')),'hex')
$$;

CREATE FUNCTION refund_reservation_vat_delta(captured numeric,captured_vat numeric,prior_gross numeric,requested_gross numeric)
 RETURNS numeric LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT CASE WHEN captured=0 THEN 0
  ELSE div(captured_vat*(prior_gross+requested_gross)+div(captured,2),captured)
    -div(captured_vat*prior_gross+div(captured,2),captured)
 END
$$;

-- Recompute the complete bounded journal with numeric intermediates; imported
-- malformed snapshots, non-integral JSON, missing events and stale proof fail closed.
CREATE FUNCTION refund_reservation_history_exact(wanted_order text) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; q jsonb; line jsonb; source jsonb; part jsonb;
 r refund_reservations%ROWTYPE; n integer:=0; i integer; count_lines integer;
 prior numeric[]; gross numeric; net numeric; vat numeric; captured numeric; captured_net numeric;
 captured_vat numeric; expected_vat numeric; sum_g numeric; sum_n numeric; sum_v numeric;
 source_g numeric:=0; source_n numeric:=0; source_v numeric:=0; last_at bigint:=0; sku text;
 seen_recognitions text[]:='{}'; original_attempt text; event_count integer;
BEGIN
 SELECT * INTO owned FROM checkout_orders WHERE id=wanted_order;
 IF owned.id IS NULL OR octet_length(owned.snapshot::text)>65536 THEN RETURN false; END IF;
 q=owned.snapshot->'quote';
 IF jsonb_typeof(q->'lines') IS DISTINCT FROM 'array' OR q->>'currency' IS DISTINCT FROM 'EUR'
  OR jsonb_array_length(q->'lines') NOT BETWEEN 1 AND 20 THEN RETURN false; END IF;
 count_lines=jsonb_array_length(q->'lines'); prior=array_fill(0::numeric,ARRAY[count_lines+1]);
 -- Verify the captured amounts independently and require distinct ordered SKU identities.
 FOR i IN 0..count_lines LOOP
  IF i=count_lines THEN source=q->'shipping';
  ELSE
   line=q->'lines'->i; source=line->'amounts'; sku=line->>'sku';
   IF sku IS NULL OR sku='' OR octet_length(sku)>128 OR EXISTS(
    SELECT 1 FROM jsonb_array_elements(q->'lines') WITH ORDINALITY x(v,j)
     WHERE j<=i AND v->>'sku'=sku) THEN RETURN false; END IF;
  END IF;
  captured=(source->>'gross_minor')::numeric; captured_net=(source->>'net_minor')::numeric; captured_vat=(source->>'vat_minor')::numeric;
  IF captured IS NULL OR captured_net IS NULL OR captured_vat IS NULL
   OR captured NOT BETWEEN 0 AND 9007199254740991 OR captured_net NOT BETWEEN 0 AND captured OR captured_vat NOT BETWEEN 0 AND captured
   OR source IS DISTINCT FROM jsonb_build_object('gross_minor',captured,'net_minor',captured_net,'vat_minor',captured_vat)
   OR trunc(captured)<>captured OR trunc(captured_net)<>captured_net OR trunc(captured_vat)<>captured_vat
   OR captured_net+captured_vat<>captured THEN RETURN false; END IF;
  source_g=source_g+captured; source_n=source_n+captured_net; source_v=source_v+captured_vat;
 END LOOP;
 IF source_g NOT BETWEEN 1 AND 9007199254740991 OR q->'total' IS DISTINCT FROM
  jsonb_build_object('gross_minor',source_g,'net_minor',source_n,'vat_minor',source_v) THEN RETURN false; END IF;
 FOR r IN SELECT * FROM refund_reservations WHERE order_id=wanted_order ORDER BY generation LIMIT 17 LOOP
  n=n+1;
  IF n=1 THEN original_attempt=r.attempt_id; END IF;
  IF n>16 OR r.generation<>n OR r.reserved_at<last_at OR r.reserved_at>9007199254740991
   OR r.id !~ '^[a-f0-9]{32}$' OR r.event_id !~ '^[a-f0-9]{32}$'
   OR r.effect_key !~ '^[a-zA-Z0-9-]{1,128}$' OR r.reason='' OR btrim(r.reason)<>r.reason
   OR octet_length(r.reason)>240 OR r.reason ~ '[[:cntrl:]]' OR r.transaction_id IS NULL
   OR NOT EXISTS(SELECT 1 FROM staff_accounts WHERE id=r.actor_id)
   OR r.attempt_id IS DISTINCT FROM original_attempt OR octet_length(r.allocation::text)>16384
   OR (NOT r.recognition_id=ANY(seen_recognitions) AND NOT late_stock_received(r.order_id,r.attempt_id,r.recognition_id,r.reserved_at,false))
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=r.event_id AND e.aggregate_id=r.order_id
    AND e.kind='order.refund_reserved.v1' AND e.created_at=r.reserved_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,
     'reservation_id',r.id,'reservation_sha256',refund_reservation_digest(r)))
   OR jsonb_typeof(r.allocation->'lines') IS DISTINCT FROM 'array'
   OR jsonb_array_length(r.allocation->'lines')<>count_lines
   OR r.allocation IS DISTINCT FROM jsonb_build_object('schema_version',1,'lines',r.allocation->'lines',
     'shipping',r.allocation->'shipping','total',r.allocation->'total') THEN RETURN false; END IF;
  seen_recognitions=array_append(seen_recognitions,r.recognition_id);
  sum_g=0;sum_n=0;sum_v=0;
  FOR i IN 0..count_lines LOOP
   IF i=count_lines THEN source=q->'shipping'; part=r.allocation->'shipping';
   ELSE source=q->'lines'->i->'amounts'; part=r.allocation->'lines'->i; END IF;
   gross=(part->>'gross_minor')::numeric; net=(part->>'net_minor')::numeric; vat=(part->>'vat_minor')::numeric;
   captured=(source->>'gross_minor')::numeric; captured_vat=(source->>'vat_minor')::numeric;
   IF gross IS NULL OR net IS NULL OR vat IS NULL OR gross NOT BETWEEN 0 AND 9007199254740991
    OR net NOT BETWEEN 0 AND gross OR vat NOT BETWEEN 0 AND gross
    OR trunc(gross)<>gross OR trunc(net)<>net OR trunc(vat)<>vat OR net+vat<>gross
    OR prior[i+1]+gross>captured THEN RETURN false; END IF;
   IF i=count_lines THEN
    IF part IS DISTINCT FROM jsonb_build_object('gross_minor',gross,'net_minor',net,'vat_minor',vat) THEN RETURN false; END IF;
   ELSE
    IF part IS DISTINCT FROM jsonb_build_object('sku',q->'lines'->i->>'sku','gross_minor',gross,'net_minor',net,'vat_minor',vat) THEN RETURN false; END IF;
   END IF;
   expected_vat=refund_reservation_vat_delta(captured,captured_vat,prior[i+1],gross);
   IF vat<>expected_vat THEN RETURN false; END IF;
   prior[i+1]=prior[i+1]+gross; sum_g=sum_g+gross;sum_n=sum_n+net;sum_v=sum_v+vat;
  END LOOP;
  IF sum_g NOT BETWEEN 1 AND 9007199254740991 OR r.allocation->'total' IS DISTINCT FROM
   jsonb_build_object('gross_minor',sum_g,'net_minor',sum_n,'vat_minor',sum_v) THEN RETURN false; END IF;
  last_at=r.reserved_at;
 END LOOP;
 SELECT count(*) INTO event_count FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=wanted_order
  AND kind='order.refund_reserved.v1' LIMIT 17) bounded_events;
 IF event_count<>n THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch OR division_by_zero THEN RETURN false;
END;
$$;

CREATE FUNCTION guard_refund_reservation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE previous refund_reservations%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Refund reservations are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Refund reservation requires read committed' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.reserved_at,true) THEN
  RAISE EXCEPTION 'Refund reservation requires current captured evidence' USING ERRCODE='23514'; END IF;
 SELECT * INTO previous FROM refund_reservations WHERE order_id=NEW.order_id ORDER BY generation DESC LIMIT 1;
 IF (previous.id IS NULL AND NEW.generation<>1) OR (previous.id IS NOT NULL AND
  (NEW.generation<>previous.generation+1 OR NEW.reserved_at<previous.reserved_at OR previous.transaction_id=pg_current_xact_id())) THEN
  RAISE EXCEPTION 'Refund reservations require distinct contiguous transactions' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_refund_reservation BEFORE INSERT OR UPDATE OR DELETE ON refund_reservations FOR EACH ROW EXECUTE FUNCTION guard_refund_reservation();

CREATE FUNCTION require_complete_refund_reservation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
  OR NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.reserved_at,true)
  OR NOT refund_reservation_history_exact(NEW.order_id) THEN
  RAISE EXCEPTION 'Refund reservation requires exact current source and reciprocal event' USING ERRCODE='23514'; END IF;
 PERFORM pg_advisory_xact_lock(1936876902,NEW.fence_key);
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_refund_reservation AFTER INSERT ON refund_reservations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_refund_reservation();

-- Sealing forbids changing the current financial proof after an early SET CONSTRAINTS.
CREATE FUNCTION guard_refund_reservation_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE item record; r refund_reservations%ROWTYPE; attempts text[]:='{}'; orders text[]:='{}';
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
  AND mode='ExclusiveLock' AND granted AND classid=1936876902::oid AND objsubid=2 LOOP
  SELECT * INTO r FROM refund_reservations WHERE fence_key=item.key;
  IF r.id IS NOT NULL AND (r.attempt_id=ANY(attempts) OR r.order_id=ANY(orders)) THEN
   RAISE EXCEPTION 'Completed refund reservation source cannot change in this transaction' USING ERRCODE='23514'; END IF;
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END;
$$;
CREATE TRIGGER aa_refund_reservation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_observations FOR EACH ROW EXECUTE FUNCTION guard_refund_reservation_seal();
CREATE TRIGGER aa_refund_reservation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_identity_resolutions FOR EACH ROW EXECUTE FUNCTION guard_refund_reservation_seal();
CREATE TRIGGER aa_refund_reservation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognitions FOR EACH ROW EXECUTE FUNCTION guard_refund_reservation_seal();
CREATE TRIGGER aa_refund_reservation_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognition_current FOR EACH ROW EXECUTE FUNCTION guard_refund_reservation_seal();

DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind)
 WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_recovery_decided.v1','order.refund_reserved.v1');
CREATE INDEX commerce_outbox_refund_reservation_order ON commerce_outbox(aggregate_id) WHERE kind='order.refund_reserved.v1';
-- Existing recognition proofs locate exact immutable observation events by these
-- fields. Refund history can revisit up to sixteen distinct observation prefixes.
CREATE INDEX commerce_outbox_observation_exact ON commerce_outbox(aggregate_id,created_at,payload)
 WHERE kind='payment.observation_recorded.v1';
CREATE UNIQUE INDEX commerce_outbox_refund_reservation_identity ON commerce_outbox((payload->>'reservation_id'))
 WHERE kind='order.refund_reserved.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1','order.stock_late_allocated.v1','order.stock_recovery_review.v1','order.stock_recovery_decided.v1','order.refund_reserved.v1'));

CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE; settlement order_stock_settlements%ROWTYPE;
 allocation late_stock_allocations%ROWTYPE; review late_stock_review_cases%ROWTYPE; decision late_stock_dispositions%ROWTYPE;
 reservation refund_reservations%ROWTYPE;
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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
