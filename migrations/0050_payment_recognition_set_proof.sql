-- Preserve the recognition custody proof while evaluating its bounded prefix as a set.
CREATE OR REPLACE FUNCTION payment_recognition_source(r payment_recognitions, require_current boolean)
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
 -- One bounded statement proves the entire captured prefix against a single
 -- fresh READ COMMITTED snapshot. The caller holds the attempt lock first.
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
 IF invalid_observation THEN RETURN false; END IF;
 IF seen<>r.through_observation OR (require_current AND EXISTS(
   SELECT 1 FROM payment_observations WHERE attempt_id=a.id AND revision>r.through_observation))
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
