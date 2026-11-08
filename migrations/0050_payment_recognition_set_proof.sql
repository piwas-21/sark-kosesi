-- Internal proof reuse: a non-null watermark is valid only after a full source
-- proof of that prefix for the same original order, attempt and profile under
-- the same attempt lock in this transaction. It is not standalone authority.
CREATE FUNCTION payment_recognition_source_after_prefix(r payment_recognitions, require_current boolean, proved_through bigint)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE a payment_attempts%ROWTYPE; owned checkout_orders%ROWTYPE;
 d payment_dispatch_starts%ROWTYPE; resolution payment_identity_resolutions%ROWTYPE;
 expected jsonb; profile jsonb;
 binding jsonb; summary jsonb; gross bigint; seen bigint; invalid_observation boolean;
BEGIN
 SELECT * INTO a FROM payment_attempts WHERE id=r.attempt_id;
 SELECT * INTO owned FROM checkout_orders WHERE id=r.order_id;
 SELECT * INTO d FROM payment_dispatch_starts WHERE attempt_id=r.attempt_id;
 SELECT * INTO resolution FROM payment_identity_resolutions WHERE id=r.resolution_id;
 IF a.id IS NULL OR owned.id IS NULL OR d.id IS NULL OR resolution.id IS NULL
  OR r.order_id IS DISTINCT FROM a.order_id OR a.basket_id IS DISTINCT FROM owned.basket_id
  OR a.created_at<owned.created_at OR a.dispatch_before<>owned.hold_expires_at
  OR d.started_at<a.created_at OR d.started_at>=a.dispatch_before
  OR r.through_observation NOT BETWEEN 0 AND 1000
  OR resolution.attempt_id IS DISTINCT FROM a.id
  OR resolution.through_observation IS DISTINCT FROM r.through_observation
  OR r.recorded_at<resolution.recorded_at OR resolution.recorded_at<d.started_at
  OR r.currency IS DISTINCT FROM 'EUR'
  OR a.id !~ '^[a-f0-9]{32}$' OR owned.id !~ '^[a-f0-9]{32}$' OR d.id !~ '^[a-f0-9]{32}$'
  OR octet_length(a.snapshot::text)>16384 OR octet_length(a.configuration::text)>8192
  OR octet_length(a.configuration_material)>8192 OR octet_length(a.request_body)>8192
  OR octet_length(owned.snapshot::text)>65536 OR octet_length(resolution.summary::text)>524288
 THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM guest_quote_snapshots q WHERE q.id=owned.quote_id
   AND q.basket_id=owned.basket_id AND q.basket_revision=owned.basket_revision
   AND owned.snapshot->'quote'=q.snapshot) THEN RETURN false; END IF;
 gross=(owned.snapshot->'quote'->'total'->>'gross_minor')::bigint;
 profile=a.configuration->'profile';
 IF profile IS DISTINCT FROM jsonb_build_object('id',profile->>'id','credential_fingerprint',profile->>'credential_fingerprint','mode','test')
  OR a.configuration IS DISTINCT FROM jsonb_build_object('schema_version',1,'profile',profile,
    'method',a.configuration->>'method','merchant_origin',a.configuration->>'merchant_origin',
    'return_url',a.configuration->>'return_url','webhook_url',a.configuration->>'webhook_url')
  OR coalesce(a.configuration->>'method' NOT IN ('ideal','bancontact','creditcard','paypal'),true)
  OR a.configuration_material::jsonb IS DISTINCT FROM jsonb_build_array('piwas-mollie-test-configuration-v1',
    profile->>'id',profile->>'credential_fingerprint','test',a.configuration->>'method',
    a.configuration->>'merchant_origin',a.configuration->>'return_url',a.configuration->>'webhook_url')
 THEN RETURN false; END IF;
 binding=jsonb_build_object('id',a.id,'order_id',owned.id,'basket_id',owned.basket_id,
   'stock_group_id',owned.stock_group_id,'profile',profile,
   'configuration_sha256',encode(sha256(convert_to(a.configuration_material,'UTF8')),'hex'),
   'currency','EUR','amount_minor',gross,'created_at',a.created_at,'dispatch_before',a.dispatch_before);
 IF gross IS NULL OR gross NOT BETWEEN 1 AND 9007199254740991 OR r.amount_minor IS DISTINCT FROM gross
  OR a.snapshot IS DISTINCT FROM jsonb_build_object('schema_version',1,'binding',binding,
     'request_sha256',encode(sha256(convert_to(a.request_body,'UTF8')),'hex'))
  OR a.idempotency_key IS DISTINCT FROM 'piwas-test-payment-'||a.id
  OR a.intent IS DISTINCT FROM jsonb_build_object('order_id',a.order_id,'confirmed',true)
  OR profile->>'mode' IS DISTINCT FROM 'test'
  OR coalesce(profile->>'id' !~ '^pfl_.+$',true) OR octet_length(profile->>'id')>256
  OR coalesce(profile->>'credential_fingerprint' !~ '^[a-f0-9]{64}$',true)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
    AND e.kind='checkout.order_created.v1' AND e.created_at=owned.created_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id))
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
    AND e.kind='payment.attempt_prepared.v1' AND e.created_at=a.created_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',a.id))
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
    AND e.kind='payment.reconciliation_required.v1' AND e.created_at=d.started_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',a.id,'dispatch_id',d.id))
 THEN RETURN false; END IF;
 expected=jsonb_build_object('amount',jsonb_build_object('currency','EUR',
   'value',(gross/100)::text||'.'||lpad((gross%100)::text,2,'0')),
   'description','Piwas order '||owned.id,'redirectUrl',(a.configuration->>'return_url')||
     '?order_id='||owned.id||'&attempt_id='||a.id,'webhookUrl',a.configuration->>'webhook_url',
   'method',a.configuration->>'method','sequenceType','oneoff',
   'metadata',jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',a.id));
 IF a.request_body::jsonb IS DISTINCT FROM expected THEN RETURN false; END IF;
 -- With no prior proof, validate the entire bounded prefix in one fresh
 -- READ COMMITTED statement after the separate attempt lock.
 IF proved_through IS NULL THEN
  WITH ordered AS MATERIALIZED (
   SELECT o.*, row_number() OVER (ORDER BY o.revision) AS position,
    lag(o.recorded_at,1,d.started_at) OVER (ORDER BY o.revision) AS previous_at
   FROM payment_observations o WHERE o.attempt_id=a.id
    AND o.revision<=r.through_observation ORDER BY o.revision LIMIT 1001
  )
  SELECT count(*),coalesce(bool_or(
    o.position>1000 OR o.revision<>o.position OR o.recorded_at<o.previous_at
    OR o.recorded_at>resolution.recorded_at OR octet_length(o.snapshot::text)>8192
    OR o.snapshot IS DISTINCT FROM jsonb_build_object('schema_version',1,'id',o.id,'attempt_id',a.id,
      'effect_key',o.effect_key,'revision',o.revision,'recorded_at',o.recorded_at,
      'intent',jsonb_build_object('payment',jsonb_build_object('resource','payment','id',o.provider_id,
        'profile_id',profile->>'id','mode','test','currency','EUR','amount_minor',gross,
        'metadata_schema_version',1,'order_id',owned.id,'attempt_id',a.id,'status',o.status),
        'credential_fingerprint',o.credential_fingerprint))
    OR o.id !~ '^[a-f0-9]{32}$' OR o.effect_key !~ '^[a-zA-Z0-9-]{1,128}$'
    OR o.credential_fingerprint !~ '^[a-f0-9]{64}$'
    OR o.provider_id !~ '^tr_.+$' OR octet_length(o.provider_id)>256
    OR btrim(o.provider_id)<>o.provider_id OR o.provider_id ~ '[[:cntrl:]]'
    OR o.status NOT IN ('open','pending','authorized','paid','canceled','expired','failed')
    OR e.id IS NULL),false)
  INTO seen,invalid_observation
  FROM ordered o LEFT JOIN commerce_outbox e ON e.kind='payment.observation_recorded.v1'
   AND e.payload->>'observation_id'=o.id AND e.aggregate_id=owned.id AND e.created_at=o.recorded_at
   AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',a.id,'observation_id',o.id);
  IF invalid_observation OR seen<>r.through_observation THEN RETURN false; END IF;
 ELSE
  -- Monotonicity was established by the full proof. Each older resolution
  -- must still be no earlier than the final observation in its own prefix.
  IF r.through_observation IS NULL OR r.through_observation<0
   OR r.through_observation>proved_through OR proved_through>1000
   OR (r.through_observation>0 AND NOT EXISTS(
     SELECT 1 FROM payment_observations o WHERE o.attempt_id=a.id
      AND o.revision=r.through_observation AND o.recorded_at<=resolution.recorded_at))
  THEN RETURN false; END IF;
 END IF;
 IF require_current AND EXISTS(
   SELECT 1 FROM payment_observations WHERE attempt_id=a.id AND revision>r.through_observation)
 THEN RETURN false; END IF;
 summary=payment_identity_summary(a.id,r.through_observation);
 IF resolution.summary IS DISTINCT FROM summary
  OR resolution.id !~ '^[a-f0-9]{32}$' OR resolution.effect_key !~ '^[a-zA-Z0-9-]{1,128}$'
  OR resolution.revision NOT BETWEEN 1 AND 9007199254740991
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
    AND e.kind='payment.identity_resolved.v1' AND e.created_at=resolution.recorded_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',a.id,'resolution_id',resolution.id))
  OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(summary->'provider_ids') i
    WHERE NOT EXISTS(SELECT 1 FROM payment_provider_identity_claims c
      JOIN payment_identity_resolutions initial ON initial.id=c.initial_resolution_id
      WHERE c.profile_id=profile->>'id' AND c.provider_id=i AND c.attempt_id=a.id
        AND initial.attempt_id=a.id AND initial.revision<=resolution.revision
        AND initial.through_observation<=resolution.through_observation
        AND initial.summary->'provider_ids' ? i
        AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
          AND e.kind='payment.identity_resolved.v1' AND e.created_at=initial.recorded_at
          AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,
            'attempt_id',a.id,'resolution_id',initial.id))))
  OR r.state IS DISTINCT FROM (CASE WHEN jsonb_array_length(summary->'provider_ids')>1 THEN 'needs_review'
     WHEN jsonb_array_length(summary->'paid_provider_ids')=1 THEN 'received' ELSE 'unconfirmed' END)
 THEN RETURN false; END IF;
 RETURN true;
END;
$$;

-- The established native entry always performs the complete proof.
CREATE OR REPLACE FUNCTION payment_recognition_source(r payment_recognitions, require_current boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 RETURN payment_recognition_source_after_prefix(r,require_current,NULL);
END;
$$;

-- Prove the greatest bounded, same-attempt recognition prefix once.
CREATE OR REPLACE FUNCTION refund_reservation_history_exact(wanted_order text) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; q jsonb; line jsonb; source jsonb; part jsonb;
 r refund_reservations%ROWTYPE; n integer:=0; i integer; count_lines integer;
 prior numeric[]; gross numeric; net numeric; vat numeric; captured numeric; captured_net numeric;
 captured_vat numeric; expected_vat numeric; sum_g numeric; sum_n numeric; sum_v numeric;
 source_g numeric:=0; source_n numeric:=0; source_v numeric:=0; last_at bigint:=0; sku text;
 seen_recognitions text[]:='{}'; original_attempt text; event_count integer;
 recognition payment_recognitions%ROWTYPE; greatest payment_recognitions%ROWTYPE;
 proved_through bigint; bounded_count integer:=0;
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
 -- Evidence is append-only; write guards separately claim the original attempt.
 -- Capture the bounded set first and prove its greatest referenced prefix.
 SELECT id INTO original_attempt FROM payment_attempts WHERE order_id=wanted_order;
 FOR r IN SELECT * FROM refund_reservations WHERE order_id=wanted_order ORDER BY generation LIMIT 17 LOOP
  bounded_count=bounded_count+1;
  IF bounded_count>16 OR original_attempt IS NULL
   OR r.attempt_id IS DISTINCT FROM original_attempt THEN RETURN false; END IF;
  SELECT * INTO recognition FROM payment_recognitions WHERE id=r.recognition_id;
  IF recognition.id IS NULL OR recognition.order_id IS DISTINCT FROM wanted_order
   OR recognition.attempt_id IS DISTINCT FROM original_attempt OR recognition.state IS DISTINCT FROM 'received'
   OR r.reserved_at<recognition.recorded_at OR r.reserved_at<owned.created_at
   OR r.reserved_at NOT BETWEEN 0 AND 9007199254740991 THEN RETURN false; END IF;
  IF greatest.id IS NULL OR recognition.through_observation>greatest.through_observation THEN
   greatest=recognition;
  END IF;
 END LOOP;
 IF bounded_count>0 THEN
  IF greatest.id IS NULL OR NOT payment_recognition_source(greatest,false) THEN RETURN false; END IF;
  proved_through=greatest.through_observation;
 END IF;
 FOR r IN SELECT * FROM refund_reservations WHERE order_id=wanted_order ORDER BY generation LIMIT 17 LOOP
  n=n+1;
  IF n>16 OR r.generation<>n OR r.reserved_at<last_at OR r.reserved_at>9007199254740991
   OR r.id !~ '^[a-f0-9]{32}$' OR r.event_id !~ '^[a-f0-9]{32}$'
   OR r.effect_key !~ '^[a-zA-Z0-9-]{1,128}$' OR r.reason='' OR btrim(r.reason)<>r.reason
   OR octet_length(r.reason)>240 OR r.reason ~ '[[:cntrl:]]' OR r.transaction_id IS NULL
   OR NOT EXISTS(SELECT 1 FROM staff_accounts WHERE id=r.actor_id)
   OR r.attempt_id IS DISTINCT FROM original_attempt OR octet_length(r.allocation::text)>16384
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=r.event_id AND e.aggregate_id=r.order_id
    AND e.kind='order.refund_reserved.v1' AND e.created_at=r.reserved_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,
     'reservation_id',r.id,'reservation_sha256',refund_reservation_digest(r)))
   OR jsonb_typeof(r.allocation->'lines') IS DISTINCT FROM 'array'
   OR jsonb_array_length(r.allocation->'lines')<>count_lines
   OR r.allocation IS DISTINCT FROM jsonb_build_object('schema_version',1,'lines',r.allocation->'lines',
     'shipping',r.allocation->'shipping','total',r.allocation->'total') THEN RETURN false; END IF;
  IF NOT r.recognition_id=ANY(seen_recognitions) THEN
   SELECT * INTO recognition FROM payment_recognitions WHERE id=r.recognition_id;
   IF recognition.id IS NULL OR recognition.order_id IS DISTINCT FROM wanted_order
    OR recognition.attempt_id IS DISTINCT FROM original_attempt OR recognition.state IS DISTINCT FROM 'received'
    OR r.reserved_at<recognition.recorded_at OR r.reserved_at<owned.created_at
    OR r.reserved_at NOT BETWEEN 0 AND 9007199254740991
    OR NOT payment_recognition_source_after_prefix(recognition,false,proved_through)
    OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
      AND e.kind='payment.recognized.v1' AND e.created_at=recognition.recorded_at
      AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,
       'attempt_id',recognition.attempt_id,'recognition_id',recognition.id,
       'recognition_sha256',payment_recognition_digest(recognition))) THEN RETURN false; END IF;
  END IF;
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
 IF n<>bounded_count THEN RETURN false; END IF;
 SELECT count(*) INTO event_count FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=wanted_order
  AND kind='order.refund_reserved.v1' LIMIT 17) bounded_events;
 IF event_count<>n THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch OR division_by_zero THEN RETURN false;
END;
$$;
