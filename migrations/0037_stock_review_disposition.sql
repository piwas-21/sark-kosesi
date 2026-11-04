-- No grant is installed by this migration. Authentication and fresh MFA are
-- enforced by the trusted service; native custody still binds an existing actor.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CHECK(capability IN
 ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review','promotion.manage','promotion.review','payment.reconcile','stock.review','stock.recover'));

-- Existing cases predate this migration and therefore have NULL here. New
-- cases are tagged by the database so a stop cannot authorize its own retry.
ALTER TABLE late_stock_review_cases ADD COLUMN admission_transaction_id xid8;

CREATE TABLE late_stock_dispositions (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 case_id text NOT NULL REFERENCES late_stock_review_cases(id),
 case_digest text NOT NULL CHECK(case_digest ~ '^[a-f0-9]{64}$'),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 actor_id text NOT NULL REFERENCES staff_accounts(id),
 effect_key text NOT NULL UNIQUE CHECK(effect_key ~ '^[a-f0-9]{32}$'),
 generation integer NOT NULL CHECK(generation BETWEEN 1 AND 3),
 reason text NOT NULL CHECK(reason=btrim(reason) AND reason !~ '(^[[:space:]]|[[:space:]]$)' AND octet_length(reason) BETWEEN 1 AND 240),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 outcome text NOT NULL CHECK(outcome IN ('allocated','still_scarce')),
 allocation_id text UNIQUE REFERENCES late_stock_allocations(id) DEFERRABLE INITIALLY DEFERRED,
 footprint jsonb,
 transaction_id xid8 NOT NULL,
 UNIQUE(case_id,generation),
 CHECK((outcome='allocated' AND allocation_id IS NOT NULL AND footprint IS NULL)
    OR (outcome='still_scarce' AND allocation_id IS NULL AND footprint IS NOT NULL AND jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=2097152))
);
ALTER TABLE late_stock_allocations ADD COLUMN disposition_id text UNIQUE REFERENCES late_stock_dispositions(id) DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE late_stock_allocations ADD COLUMN disposition_transaction_id xid8;

CREATE FUNCTION late_stock_disposition_digest(d late_stock_dispositions) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT CASE WHEN d.footprint IS NOT NULL AND octet_length(d.footprint::text)>2097152 THEN NULL ELSE encode(sha256(convert_to(jsonb_build_object('id',d.id,'case_id',d.case_id,'case_digest',d.case_digest,
  'order_id',d.order_id,'attempt_id',d.attempt_id,'recognition_id',d.recognition_id,'actor_id',d.actor_id,
  'effect_key',d.effect_key,'generation',d.generation,'reason',d.reason,'recorded_at',d.recorded_at,
  'outcome',d.outcome,'allocation_id',d.allocation_id,'footprint',d.footprint)::text,'UTF8')),'hex') END
$$;

-- This narrow predicate allows the old path to observe only a preclaimed,
-- same-transaction successful disposition. Full proof is deferred to commit.
CREATE FUNCTION late_stock_disposition_linked(wanted_order text,wanted_allocation text) RETURNS boolean LANGUAGE sql VOLATILE AS $$
 SELECT EXISTS(SELECT 1 FROM late_stock_dispositions d
  JOIN late_stock_review_cases c ON c.id=d.case_id
  JOIN late_stock_allocations a ON a.id=d.allocation_id
  WHERE d.outcome='allocated' AND d.order_id=wanted_order AND d.allocation_id=wanted_allocation
   AND c.order_id=d.order_id AND c.attempt_id=d.attempt_id AND c.reason='component_scarcity'
   AND d.case_digest= (CASE WHEN octet_length(c.footprint::text)<=2097152 THEN late_stock_review_digest(c) ELSE NULL END) AND a.order_id=d.order_id AND a.attempt_id=d.attempt_id
   AND a.recognition_id=d.recognition_id AND a.allocated_at=d.recorded_at
   AND a.disposition_id=d.id AND a.disposition_transaction_id=d.transaction_id)
$$;
CREATE OR REPLACE FUNCTION late_stock_review_exact(s late_stock_review_cases,admission boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; expected_reason text; scarce boolean;
BEGIN
 IF octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF s.id IS NULL OR s.actor IS DISTINCT FROM 'late-stock-review' OR (s.fence_key IS NULL OR s.fence_key<=0)
  OR octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' OR (s.footprint->'schema_version')::text IS DISTINCT FROM '1'
  OR s.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'original',s.footprint->'original','components',s.footprint->'components') THEN RETURN false; END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=s.order_id;
 IF owned.id IS NULL OR owned.stock_group_id IS DISTINCT FROM s.group_id
  OR NOT late_stock_received(s.order_id,s.attempt_id,s.recognition_id,s.assessed_at,admission)
  OR NOT late_stock_original_exact(s.order_id,s.footprint->'original',NOT admission)
  OR NOT late_stock_components_exact(s.footprint->'original',s.footprint->'components',false,CASE WHEN admission THEN 'before' ELSE 'none' END)
  OR EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=s.order_id AND
   (admission OR a.kind<>'late' OR NOT late_stock_disposition_linked(s.order_id,a.receipt_id))) THEN RETURN false; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state'='consumed') THEN
  expected_reason='prior_consumption_without_allocation';
 ELSIF EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state'='released') THEN
  expected_reason='release_intent_unresolved';
 ELSE
  IF s.assessed_at<owned.hold_expires_at THEN RETURN false; END IF;
  SELECT EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'components') c
   WHERE (c->'before'->>'on_hand')::numeric-(c->'before'->>'quarantine')::bigint-(c->'before'->>'reserved')::bigint+(c->>'active')::bigint<(c->>'demand')::bigint) INTO scarce;
  IF NOT scarce THEN RETURN false; END IF;expected_reason='component_scarcity';
 END IF;
 IF s.reason IS DISTINCT FROM expected_reason THEN RETURN false; END IF;
 IF admission AND (s.footprint->'original')::text IS DISTINCT FROM late_stock_original(s.order_id)::text THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_review_history_exact(s late_stock_review_cases) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF s.id IS NULL OR octet_length(s.footprint::text)>2097152 OR NOT late_stock_review_exact(s,false) THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'
  AND e.created_at=s.assessed_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,
   'case_id',s.id,'case_sha256',late_stock_review_digest(s)));
END;
$$;

CREATE OR REPLACE FUNCTION late_stock_allocation_exact(s late_stock_allocations,current_balances boolean,current_financial boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; m jsonb; fresh_ids text[]; active_ids text[];
 last_expiry bigint; first_reserve bigint; last_reserve bigint; first_consume bigint;
BEGIN
 IF octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF s.id IS NULL OR s.actor IS DISTINCT FROM 'late-payment-allocation' OR (s.fence_key IS NULL OR s.fence_key<=0)
  OR octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' OR (s.footprint->'schema_version')::text IS DISTINCT FROM '1'
  OR s.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'original',s.footprint->'original',
    'fresh',s.footprint->'fresh','components',s.footprint->'components') THEN RETURN false; END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=s.order_id;
 IF owned.id IS NULL OR owned.stock_group_id IS DISTINCT FROM s.original_group_id OR s.original_group_id=s.fresh_group_id
  OR s.expires_at NOT BETWEEN 1 AND 9007199254740991 OR s.allocated_at<s.expires_at-60 OR s.allocated_at>=s.expires_at
  OR s.expires_at-60<owned.hold_expires_at
  OR NOT late_stock_received(s.order_id,s.attempt_id,s.recognition_id,s.expires_at-60,current_financial)
  OR NOT late_stock_original_exact(s.order_id,s.footprint->'original',true)
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state' NOT IN ('active','expired'))
  OR EXISTS(SELECT 1 FROM stock_holds WHERE basket_group_id=s.original_group_id AND state<>'expired')
  OR NOT late_stock_fresh_exact(s,false)
  OR NOT late_stock_components_exact(s.footprint->'original',s.footprint->'components',true,CASE WHEN current_balances THEN 'after' ELSE 'none' END)
  OR NOT EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=s.order_id AND a.attempt_id=s.attempt_id AND a.kind='late' AND a.receipt_id=s.id)
  OR EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=s.order_id AND NOT late_stock_disposition_linked(s.order_id,s.id))
  OR NOT late_stock_stage_exact(s,'expiry') OR NOT late_stock_stage_exact(s,'reserve') OR NOT late_stock_stage_exact(s,'consume')
 THEN RETURN false; END IF;
 SELECT array_agg(value->>'hold_id') INTO fresh_ids FROM jsonb_array_elements(s.footprint->'fresh');
 SELECT array_agg(value->'member'->>'hold_id') INTO active_ids FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state'='active';
 SELECT max(sequence) INTO last_expiry FROM stock_movements WHERE hold_id=ANY(active_ids) AND action='expire-hold';
 SELECT min(sequence),max(sequence) INTO first_reserve,last_reserve FROM stock_movements WHERE hold_id=ANY(fresh_ids) AND action='reserve';
 SELECT min(sequence) INTO first_consume FROM stock_movements WHERE hold_id=ANY(fresh_ids) AND action='consume-hold';
 IF first_reserve IS NULL OR first_consume IS NULL OR (last_expiry IS NOT NULL AND first_reserve<=last_expiry)
  OR first_consume<=last_reserve THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE OR REPLACE FUNCTION guard_order_stock_allocation_claim() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Order allocation claims are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Order allocation requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=NEW.order_id AND
  (NEW.kind<>'late' OR NOT late_stock_disposition_linked(NEW.order_id,NEW.receipt_id)))
  OR (NEW.kind='original' AND NOT EXISTS(SELECT 1 FROM order_stock_settlements s WHERE s.id=NEW.receipt_id AND s.order_id=NEW.order_id AND s.attempt_id=NEW.attempt_id))
  OR (NEW.kind='late' AND NOT EXISTS(SELECT 1 FROM late_stock_allocations s WHERE s.id=NEW.receipt_id AND s.order_id=NEW.order_id AND s.attempt_id=NEW.attempt_id)) THEN
  RAISE EXCEPTION 'Allocation claim requires exact owning receipt and no review stop' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION order_stock_allocation_exact(a order_stock_allocations) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE original order_stock_settlements%ROWTYPE; late late_stock_allocations%ROWTYPE;
BEGIN
 IF a.order_id IS NULL OR EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=a.order_id AND
  (a.kind<>'late' OR NOT late_stock_disposition_linked(a.order_id,a.receipt_id))) THEN RETURN false; END IF;
 IF a.kind='original' THEN
  SELECT * INTO original FROM order_stock_settlements WHERE id=a.receipt_id AND order_id=a.order_id AND attempt_id=a.attempt_id;
  RETURN order_stock_original_history_exact(original)
   AND NOT EXISTS(SELECT 1 FROM late_stock_allocations WHERE order_id=a.order_id);
 ELSIF a.kind='late' THEN
  SELECT * INTO late FROM late_stock_allocations WHERE id=a.receipt_id AND order_id=a.order_id AND attempt_id=a.attempt_id;
  IF NOT late_stock_allocation_exact(late,false,false) THEN RETURN false; END IF;
  RETURN NOT EXISTS(SELECT 1 FROM order_stock_settlements WHERE order_id=a.order_id)
   AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=late.order_id AND e.kind='order.stock_late_allocated.v1'
    AND e.created_at=late.allocated_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',late.order_id,
     'allocation_id',late.id,'allocation_sha256',late_stock_digest(late)));
 END IF;
 RETURN false;
END;
$$;
CREATE OR REPLACE FUNCTION guard_late_stock_allocation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; decision late_stock_dispositions%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Late allocations are immutable' USING ERRCODE='23514'; END IF;
 IF octet_length(NEW.footprint::text)>2097152 OR jsonb_typeof(NEW.footprint) IS DISTINCT FROM 'object' THEN
  RAISE EXCEPTION 'Late allocation footprint exceeds closed envelope' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Late allocation requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT * INTO decision FROM late_stock_dispositions WHERE allocation_id=NEW.id AND outcome='allocated';
 IF decision.id IS NOT NULL THEN
  IF decision.transaction_id IS DISTINCT FROM pg_current_xact_id() OR decision.order_id IS DISTINCT FROM NEW.order_id
   OR decision.attempt_id IS DISTINCT FROM NEW.attempt_id OR decision.recognition_id IS DISTINCT FROM NEW.recognition_id
   OR decision.recorded_at IS DISTINCT FROM NEW.allocated_at THEN
   RAISE EXCEPTION 'Allocation requires its same-transaction disposition' USING ERRCODE='23514'; END IF;
  NEW.disposition_id=decision.id;NEW.disposition_transaction_id=pg_current_xact_id();
 ELSE
  NEW.disposition_id=NULL;NEW.disposition_transaction_id=NULL;
 END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=NEW.order_id;
 IF owned.id IS NULL OR owned.stock_group_id IS DISTINCT FROM NEW.original_group_id
  OR NEW.expires_at-60<owned.hold_expires_at
  OR NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.expires_at-60,true)
  OR NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.allocated_at,true)
  OR EXISTS(SELECT 1 FROM order_stock_allocations WHERE order_id=NEW.order_id)
  OR EXISTS(SELECT 1 FROM late_stock_review_cases c WHERE c.order_id=NEW.order_id AND
   NOT (decision.id IS NOT NULL AND decision.case_id=c.id AND c.reason='component_scarcity'
    AND decision.case_digest= (CASE WHEN octet_length(c.footprint::text)<=2097152 THEN late_stock_review_digest(c) ELSE NULL END))) THEN
  RAISE EXCEPTION 'Late allocation requires current received source and no prior outcome' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.original_group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.original_group_id ORDER BY id LIMIT 21 FOR UPDATE;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.fresh_group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.fresh_group_id ORDER BY id LIMIT 21 FOR UPDATE;
 IF (NEW.footprint->'schema_version')::text IS DISTINCT FROM '1' OR NEW.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'original',NEW.footprint->'original',
   'fresh',NEW.footprint->'fresh','components',NEW.footprint->'components')
  OR (NEW.footprint->'original')::text IS DISTINCT FROM late_stock_original(NEW.order_id)::text
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(NEW.footprint->'original') WHERE value->>'state' NOT IN ('active','expired'))
  OR NOT late_stock_fresh_exact(NEW,true) THEN
  RAISE EXCEPTION 'Late allocation requires exact eligible original and fresh building source' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM stock_balances WHERE component_code IN (
  SELECT line->>'component_code' FROM jsonb_array_elements(NEW.footprint->'original') m
   CROSS JOIN LATERAL jsonb_array_elements(m->'member'->'components') line)
  ORDER BY component_code LIMIT 1281 FOR UPDATE;
 IF NOT late_stock_components_exact(NEW.footprint->'original',NEW.footprint->'components',true,'before') THEN
  RAISE EXCEPTION 'Late allocation requires actual backed before and complete stage footprint' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION guard_late_stock_review() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Stock review cases are immutable' USING ERRCODE='23514'; END IF;
 NEW.admission_transaction_id=pg_current_xact_id();
 IF octet_length(NEW.footprint::text)>2097152 OR jsonb_typeof(NEW.footprint) IS DISTINCT FROM 'object' THEN
  RAISE EXCEPTION 'Stock review footprint exceeds closed envelope' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Stock review requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.group_id ORDER BY id LIMIT 21 FOR UPDATE;
 PERFORM 1 FROM stock_balances WHERE component_code IN (
  SELECT line->>'component_code' FROM jsonb_array_elements(order_stock_source(NEW.order_id)) m
   CROSS JOIN LATERAL jsonb_array_elements(m->'components') line)
  ORDER BY component_code LIMIT 1281 FOR UPDATE;
 IF NOT late_stock_review_exact(NEW,true) THEN
  RAISE EXCEPTION 'Stock review requires exact received historical assessment and actual source' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
-- Single-receipt proof uses immutable historical inputs; current availability
-- and current financial state are only required at admission/final allocation.
CREATE FUNCTION late_stock_disposition_single_exact(d late_stock_dispositions, c late_stock_review_cases, require_event boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE assessment late_stock_review_cases%ROWTYPE;
 a late_stock_allocations%ROWTYPE;
BEGIN
 IF d.id IS NULL OR c.id IS NULL OR c.reason<>'component_scarcity'
  OR d.case_digest IS DISTINCT FROM (CASE WHEN octet_length(c.footprint::text)<=2097152 THEN late_stock_review_digest(c) ELSE NULL END)
  OR (d.order_id,d.attempt_id) IS DISTINCT FROM (c.order_id,c.attempt_id)
  OR c.id IS DISTINCT FROM d.case_id OR d.recorded_at<c.assessed_at
  OR NOT late_stock_received(d.order_id,d.attempt_id,d.recognition_id,d.recorded_at,false) THEN RETURN false; END IF;
 IF d.outcome='still_scarce' THEN
  IF d.footprint IS NULL OR octet_length(d.footprint::text)>2097152 OR d.allocation_id IS NOT NULL THEN RETURN false; END IF;
  assessment=c;assessment.footprint=d.footprint;assessment.assessed_at=d.recorded_at;assessment.recognition_id=d.recognition_id;
  IF NOT late_stock_review_exact(assessment,false) THEN RETURN false; END IF;
 ELSIF d.outcome='allocated' THEN
  SELECT * INTO a FROM late_stock_allocations WHERE id=d.allocation_id;
  IF d.footprint IS NOT NULL OR a.id IS NULL OR NOT late_stock_disposition_linked(d.order_id,a.id)
   OR NOT late_stock_allocation_exact(a,false,false)
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=a.order_id AND e.kind='order.stock_late_allocated.v1'
    AND e.created_at=a.allocated_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,
     'allocation_id',a.id,'allocation_sha256',CASE WHEN octet_length(a.footprint::text)<=2097152 THEN late_stock_digest(a) ELSE NULL END)) THEN RETURN false; END IF;
 ELSE RETURN false;
 END IF;
 IF require_event AND NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=d.order_id
  AND e.kind='order.stock_recovery_decided.v1' AND e.created_at=d.recorded_at
  AND e.payload=jsonb_build_object('schema_version',1,'order_id',d.order_id,
   'decision_id',d.id,'decision_sha256',late_stock_disposition_digest(d))) THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_disposition_history_exact(d late_stock_dispositions) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE previous late_stock_dispositions%ROWTYPE; c late_stock_review_cases%ROWTYPE; n integer := 0; last_time bigint := 0;
BEGIN
 IF d.id IS NULL OR d.generation NOT BETWEEN 1 AND 3 THEN RETURN false; END IF;
 SELECT * INTO c FROM late_stock_review_cases WHERE id=d.case_id;
 IF c.id IS NULL OR NOT late_stock_review_history_exact(c) THEN RETURN false; END IF;
 FOR previous IN SELECT * FROM late_stock_dispositions WHERE case_id=d.case_id AND generation<=d.generation ORDER BY generation LIMIT 4 LOOP
  n=n+1;
  IF previous.generation<>n OR previous.recorded_at<last_time OR previous.case_digest IS DISTINCT FROM d.case_digest
   OR (n<d.generation AND previous.outcome<>'still_scarce') OR NOT late_stock_disposition_single_exact(previous,c,true)
   THEN RETURN false; END IF;
  last_time=previous.recorded_at;
 END LOOP;
 RETURN n=d.generation AND previous.id=d.id;
END;
$$;

CREATE FUNCTION guard_late_stock_disposition() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE c late_stock_review_cases%ROWTYPE; prior late_stock_dispositions%ROWTYPE; assessment late_stock_review_cases%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Stock recovery decisions are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Stock recovery requires read committed' USING ERRCODE='23514'; END IF;
 NEW.transaction_id=pg_current_xact_id();
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT * INTO c FROM late_stock_review_cases WHERE id=NEW.case_id;
 PERFORM 1 FROM basket_hold_groups WHERE id=c.group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=c.group_id ORDER BY id LIMIT 21 FOR UPDATE;
 IF c.id IS NULL OR c.admission_transaction_id=pg_current_xact_id() OR c.reason<>'component_scarcity' OR NEW.case_digest IS DISTINCT FROM (CASE WHEN octet_length(c.footprint::text)<=2097152 THEN late_stock_review_digest(c) ELSE NULL END)
  OR (NEW.order_id,NEW.attempt_id) IS DISTINCT FROM (c.order_id,c.attempt_id)
  OR NEW.recorded_at<c.assessed_at OR NOT late_stock_review_history_exact(c)
  OR NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.recorded_at,true)
  OR EXISTS(SELECT 1 FROM order_stock_allocations WHERE order_id=NEW.order_id) THEN
  RAISE EXCEPTION 'Stock recovery requires current paid source and exact scarcity case' USING ERRCODE='23514'; END IF;
 SELECT * INTO prior FROM late_stock_dispositions WHERE case_id=NEW.case_id ORDER BY generation DESC LIMIT 1;
 IF (prior.id IS NULL AND NEW.generation<>1) OR (prior.id IS NOT NULL AND
  (prior.outcome<>'still_scarce' OR prior.transaction_id=pg_current_xact_id() OR NEW.generation<>prior.generation+1 OR NEW.recorded_at<prior.recorded_at
   OR NOT late_stock_disposition_history_exact(prior))) THEN
  RAISE EXCEPTION 'Stock recovery requires contiguous valid history' USING ERRCODE='23514'; END IF;
 IF NEW.outcome='still_scarce' THEN
  IF NEW.footprint IS NULL OR octet_length(NEW.footprint::text)>2097152 THEN
   RAISE EXCEPTION 'Stock recovery requires bounded scarcity footprint' USING ERRCODE='23514'; END IF;
  PERFORM 1 FROM basket_hold_groups WHERE id=c.group_id FOR SHARE;
  PERFORM 1 FROM stock_holds WHERE basket_group_id=c.group_id ORDER BY id LIMIT 21 FOR UPDATE;
  PERFORM 1 FROM stock_balances WHERE component_code IN (
   SELECT line->>'component_code' FROM jsonb_array_elements(order_stock_source(NEW.order_id)) m
    CROSS JOIN LATERAL jsonb_array_elements(m->'components') line)
   ORDER BY component_code LIMIT 1281 FOR UPDATE;
  assessment=c;assessment.footprint=NEW.footprint;assessment.assessed_at=NEW.recorded_at;assessment.recognition_id=NEW.recognition_id;
  IF NOT late_stock_review_exact(assessment,true) THEN
   RAISE EXCEPTION 'Stock recovery requires refreshed actual scarcity' USING ERRCODE='23514'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_late_stock_disposition BEFORE INSERT OR UPDATE OR DELETE ON late_stock_dispositions FOR EACH ROW EXECUTE FUNCTION guard_late_stock_disposition();

CREATE FUNCTION require_complete_late_stock_disposition() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NOT late_stock_disposition_history_exact(NEW) THEN
  RAISE EXCEPTION 'Stock recovery requires complete reciprocal decision and event custody' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_late_stock_disposition AFTER INSERT ON late_stock_dispositions
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_late_stock_disposition();

-- Each case can have up to three decisions, with one event per decision.
DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind)
 WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1',
  'payment.identity_resolved.v1','payment.recognized.v1','order.stock_recovery_decided.v1');
CREATE UNIQUE INDEX commerce_outbox_stock_decision_identity ON commerce_outbox((payload->>'decision_id'))
 WHERE kind='order.stock_recovery_decided.v1';

ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1','order.stock_late_allocated.v1','order.stock_recovery_review.v1','order.stock_recovery_decided.v1'));

CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE; settlement order_stock_settlements%ROWTYPE;
 allocation late_stock_allocations%ROWTYPE; review late_stock_review_cases%ROWTYPE; decision late_stock_dispositions%ROWTYPE;
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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
