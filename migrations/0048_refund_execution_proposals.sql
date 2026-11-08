-- Immutable private test-mode preparation. This grants no execution authority.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CONSTRAINT staff_capabilities_capability_check CHECK(capability IN
 ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review',
  'promotion.manage','promotion.review','payment.reconcile','stock.review','stock.recover',
  'refund.prepare','refund.execution.prepare'));

CREATE TABLE refund_execution_proposals (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 fence_key integer GENERATED ALWAYS AS IDENTITY UNIQUE CHECK(fence_key>0),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 reservation_id text NOT NULL UNIQUE REFERENCES refund_reservations(id),
 reservation_sha256 text NOT NULL CHECK(reservation_sha256 ~ '^[a-f0-9]{64}$'),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 through_observation bigint NOT NULL CHECK(through_observation BETWEEN 0 AND 1000),
 profile_id text NOT NULL CHECK(payment_reversal_page_provider_id(profile_id,'pfl_')),
 payment_id text NOT NULL CHECK(payment_reversal_page_provider_id(payment_id,'tr_')),
 actor_id text NOT NULL REFERENCES staff_accounts(id),
 effect_key text NOT NULL UNIQUE CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 reason text NOT NULL CHECK(octet_length(reason)<=240 AND btrim(reason)=reason AND reason<>'' AND reason !~ '[[:cntrl:]]'),
 proposed_at bigint NOT NULL CHECK(proposed_at BETWEEN 0 AND 9007199254740991),
 allocation jsonb NOT NULL CHECK(jsonb_typeof(allocation)='object' AND octet_length(allocation::text)<=16384),
 event_id text NOT NULL UNIQUE CHECK(event_id ~ '^[a-f0-9]{32}$'),
 transaction_id xid8 NOT NULL
);
CREATE FUNCTION refund_execution_proposal_digest(p refund_execution_proposals) RETURNS text
 LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',p.id,'order_id',p.order_id,
  'attempt_id',p.attempt_id,'reservation_id',p.reservation_id,'reservation_sha256',p.reservation_sha256,
  'recognition_id',p.recognition_id,'through_observation',p.through_observation,'profile_id',p.profile_id,
  'payment_id',p.payment_id,'actor_id',p.actor_id,'effect_key',p.effect_key,'reason',p.reason,
  'proposed_at',p.proposed_at,'allocation',p.allocation,'event_id',p.event_id)::text,'UTF8')),'hex')
$$;
CREATE FUNCTION refund_execution_proposal_event(p refund_execution_proposals) RETURNS jsonb
 LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT jsonb_build_object('schema_version',1,'order_id',p.order_id,
  'proposal_id',p.id,'proposal_sha256',refund_execution_proposal_digest(p))
$$;

-- Historical verification deliberately does not require a still-current pointer.
CREATE FUNCTION refund_execution_proposal_source(p refund_execution_proposals, require_current boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE reservation refund_reservations%ROWTYPE; recognition payment_recognitions%ROWTYPE;
 original payment_recognitions%ROWTYPE; resolution payment_identity_resolutions%ROWTYPE;
 attempt payment_attempts%ROWTYPE;
BEGIN
 SELECT * INTO reservation FROM refund_reservations WHERE id=p.reservation_id;
 SELECT * INTO recognition FROM payment_recognitions WHERE id=p.recognition_id;
 SELECT * INTO original FROM payment_recognitions WHERE id=reservation.recognition_id;
 SELECT * INTO resolution FROM payment_identity_resolutions WHERE id=recognition.resolution_id;
 SELECT * INTO attempt FROM payment_attempts WHERE id=p.attempt_id;
 IF reservation.id IS NULL OR recognition.id IS NULL OR original.id IS NULL OR resolution.id IS NULL OR attempt.id IS NULL
  OR p.order_id IS DISTINCT FROM reservation.order_id OR p.attempt_id IS DISTINCT FROM reservation.attempt_id
  OR p.reservation_sha256 IS DISTINCT FROM refund_reservation_digest(reservation)
  OR p.allocation IS DISTINCT FROM reservation.allocation OR p.proposed_at<reservation.reserved_at
  OR recognition.order_id IS DISTINCT FROM p.order_id OR recognition.attempt_id IS DISTINCT FROM p.attempt_id
  OR original.order_id IS DISTINCT FROM p.order_id OR original.attempt_id IS DISTINCT FROM p.attempt_id
  OR recognition.through_observation<original.through_observation OR recognition.recorded_at<original.recorded_at
  OR p.through_observation IS DISTINCT FROM recognition.through_observation
  OR p.profile_id IS DISTINCT FROM attempt.configuration->'profile'->>'id'
  OR jsonb_array_length(resolution.summary->'provider_ids')<>1
  OR jsonb_array_length(resolution.summary->'paid_provider_ids')<>1
  OR p.payment_id IS DISTINCT FROM resolution.summary->'provider_ids'->>0
  OR p.payment_id IS DISTINCT FROM resolution.summary->'paid_provider_ids'->>0
  OR NOT EXISTS(SELECT 1 FROM payment_provider_identity_claims c
   WHERE c.profile_id=p.profile_id AND c.provider_id=p.payment_id AND c.attempt_id=p.attempt_id)
  OR NOT refund_reservation_history_exact(p.order_id)
  OR NOT late_stock_received(p.order_id,p.attempt_id,p.recognition_id,p.proposed_at,require_current)
 THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION guard_refund_execution_proposal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Refund execution proposals are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Refund execution proposal requires read committed' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_refund_execution_proposal BEFORE INSERT OR UPDATE OR DELETE ON refund_execution_proposals
 FOR EACH ROW EXECUTE FUNCTION guard_refund_execution_proposal();

CREATE FUNCTION require_complete_refund_execution_proposal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_current_xact_id()
  OR NOT refund_execution_proposal_source(NEW,true)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=NEW.event_id AND e.aggregate_id=NEW.order_id
   AND e.kind='order.refund_execution_proposed.v1' AND e.created_at=NEW.proposed_at
   AND e.payload=refund_execution_proposal_event(NEW)) THEN
  RAISE EXCEPTION 'Refund execution proposal requires exact current source and reciprocal event' USING ERRCODE='23514'; END IF;
 PERFORM pg_advisory_xact_lock(1936876903,NEW.fence_key);
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_refund_execution_proposal AFTER INSERT ON refund_execution_proposals
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_refund_execution_proposal();

-- Protect the current proof and bounded reservation history after early deferred checks.
CREATE FUNCTION guard_refund_execution_proposal_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE item record; p refund_execution_proposals%ROWTYPE; attempts text[]:='{}'; orders text[]:='{}';
BEGIN
 IF TG_TABLE_NAME IN ('payment_recognition_current','refund_reservations') THEN
  IF TG_OP<>'DELETE' THEN orders=array_append(orders,NEW.order_id); END IF;
  IF TG_OP<>'INSERT' THEN orders=array_append(orders,OLD.order_id); END IF;
 ELSE
  IF TG_OP<>'DELETE' THEN attempts=array_append(attempts,NEW.attempt_id); END IF;
  IF TG_OP<>'INSERT' THEN attempts=array_append(attempts,OLD.attempt_id); END IF;
 END IF;
 FOR item IN SELECT objid::bigint AS key FROM pg_locks WHERE locktype='advisory'
  AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND pid=pg_backend_pid()
  AND mode='ExclusiveLock' AND granted AND classid=1936876903::oid AND objsubid=2 LOOP
  SELECT * INTO p FROM refund_execution_proposals WHERE fence_key=item.key;
  IF p.id IS NOT NULL AND (p.attempt_id=ANY(attempts) OR p.order_id=ANY(orders)) THEN
   RAISE EXCEPTION 'Completed refund execution proposal source cannot change in this transaction' USING ERRCODE='23514'; END IF;
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END;
$$;
CREATE TRIGGER aa_refund_execution_proposal_seal BEFORE INSERT OR UPDATE OR DELETE ON refund_reservations FOR EACH ROW EXECUTE FUNCTION guard_refund_execution_proposal_seal();
CREATE TRIGGER aa_refund_execution_proposal_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_observations FOR EACH ROW EXECUTE FUNCTION guard_refund_execution_proposal_seal();
CREATE TRIGGER aa_refund_execution_proposal_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_identity_resolutions FOR EACH ROW EXECUTE FUNCTION guard_refund_execution_proposal_seal();
CREATE TRIGGER aa_refund_execution_proposal_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognitions FOR EACH ROW EXECUTE FUNCTION guard_refund_execution_proposal_seal();
CREATE TRIGGER aa_refund_execution_proposal_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognition_current FOR EACH ROW EXECUTE FUNCTION guard_refund_execution_proposal_seal();

DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind)
 WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_recovery_decided.v1',
  'order.refund_reserved.v1','payment.reversal_observed.v1','payment.reversal_page_recorded.v1',
  'order.refund_execution_proposed.v1');
CREATE UNIQUE INDEX commerce_outbox_refund_execution_proposal_identity ON commerce_outbox((payload->>'proposal_id'))
 WHERE kind='order.refund_execution_proposed.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN
 ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1',
  'payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1',
  'payment.recognized.v1','order.stock_settled.v1','order.stock_late_allocated.v1',
  'order.stock_recovery_review.v1','order.stock_recovery_decided.v1','order.refund_reserved.v1',
  'payment.reversal_observed.v1','payment.reversal_page_recorded.v1','order.refund_execution_proposed.v1'));

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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
