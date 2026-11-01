-- Private gross recognition; no stock, refund, invoice or fulfilment effects.
CREATE TABLE payment_recognitions (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 order_id text NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 resolution_id text NOT NULL REFERENCES payment_identity_resolutions(id),
 through_observation bigint NOT NULL CHECK(through_observation BETWEEN 0 AND 1000),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 state text NOT NULL CHECK(state IN ('unconfirmed','received','needs_review')),
 currency text NOT NULL CHECK(currency='EUR'),
 amount_minor bigint NOT NULL CHECK(amount_minor BETWEEN 1 AND 9007199254740991),
 UNIQUE(order_id,through_observation)
);
CREATE TABLE payment_recognition_current (
 order_id text PRIMARY KEY REFERENCES checkout_orders(id),
 recognition_id text UNIQUE NOT NULL REFERENCES payment_recognitions(id)
);
-- Independent event binding preserves the originally selected resolution even
-- when another valid resolution later summarizes the same observation prefix.
CREATE FUNCTION payment_recognition_digest(r payment_recognitions) RETURNS text
 LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',r.id,'order_id',r.order_id,
   'attempt_id',r.attempt_id,'resolution_id',r.resolution_id,
   'through_observation',r.through_observation,'recorded_at',r.recorded_at,
   'state',r.state,'currency',r.currency,'amount_minor',r.amount_minor)::text,'UTF8')),'hex')
$$;

-- These snapshot-sensitive proofs run only after a separate attempt lock in the
-- native write guards. VOLATILE obtains fresh RC evidence after actual lock waits.
CREATE FUNCTION payment_recognition_source(r payment_recognitions, require_current boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE a payment_attempts%ROWTYPE; owned checkout_orders%ROWTYPE;
 d payment_dispatch_starts%ROWTYPE; resolution payment_identity_resolutions%ROWTYPE;
 o payment_observations%ROWTYPE; expected jsonb; payment jsonb; profile jsonb;
 binding jsonb; summary jsonb; gross bigint; seen bigint := 0; last_at bigint;
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
 last_at=d.started_at;
 FOR o IN SELECT * FROM payment_observations WHERE attempt_id=a.id
   AND revision<=r.through_observation ORDER BY revision LIMIT 1001 LOOP
  seen=seen+1;
  payment=jsonb_build_object('resource','payment','id',o.provider_id,
    'profile_id',profile->>'id','mode','test','currency','EUR','amount_minor',gross,
    'metadata_schema_version',1,'order_id',owned.id,'attempt_id',a.id,'status',o.status);
  expected=jsonb_build_object('schema_version',1,'id',o.id,'attempt_id',a.id,
    'effect_key',o.effect_key,'revision',o.revision,'recorded_at',o.recorded_at,
    'intent',jsonb_build_object('payment',payment,'credential_fingerprint',o.credential_fingerprint));
  IF seen>1000 OR o.revision<>seen OR o.recorded_at<last_at
   OR o.recorded_at>resolution.recorded_at OR octet_length(o.snapshot::text)>8192 OR o.snapshot IS DISTINCT FROM expected
   OR o.id !~ '^[a-f0-9]{32}$' OR o.effect_key !~ '^[a-zA-Z0-9-]{1,128}$'
   OR o.credential_fingerprint !~ '^[a-f0-9]{64}$'
   OR o.provider_id !~ '^tr_.+$' OR octet_length(o.provider_id)>256
   OR btrim(o.provider_id)<>o.provider_id OR o.provider_id ~ '[[:cntrl:]]'
   OR o.status NOT IN ('open','pending','authorized','paid','canceled','expired','failed')
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
     AND e.kind='payment.observation_recorded.v1' AND e.created_at=o.recorded_at
     AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',a.id,'observation_id',o.id))
  THEN RETURN false; END IF;
  last_at=o.recorded_at;
 END LOOP;
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
CREATE FUNCTION guard_payment_recognition() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE previous payment_recognitions%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Payment recognition is immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Recognition requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF NOT payment_recognition_source(NEW,true) THEN
  RAISE EXCEPTION 'Recognition requires complete current authenticated evidence' USING ERRCODE='23514'; END IF;
 SELECT * INTO previous FROM payment_recognitions WHERE order_id=NEW.order_id ORDER BY through_observation DESC LIMIT 1;
 IF previous.id IS NOT NULL AND (NEW.through_observation<=previous.through_observation OR NEW.recorded_at<previous.recorded_at) THEN
  RAISE EXCEPTION 'Recognition history cannot rewind' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_recognition BEFORE INSERT OR UPDATE OR DELETE ON payment_recognitions FOR EACH ROW EXECUTE FUNCTION guard_payment_recognition();
CREATE FUNCTION guard_payment_recognition_current() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE wanted payment_recognitions%ROWTYPE; previous payment_recognitions%ROWTYPE; attempt text;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Recognition pointer cannot detach' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Recognition pointer requires read committed' USING ERRCODE='23514'; END IF;
 SELECT id INTO attempt FROM payment_attempts WHERE order_id=NEW.order_id;
 PERFORM 1 FROM payment_attempts WHERE id=attempt FOR UPDATE;
 SELECT * INTO wanted FROM payment_recognitions WHERE id=NEW.recognition_id;
 IF wanted.order_id IS DISTINCT FROM NEW.order_id OR NOT payment_recognition_source(wanted,true)
  OR EXISTS(SELECT 1 FROM payment_recognitions WHERE order_id=NEW.order_id AND through_observation>wanted.through_observation) THEN
  RAISE EXCEPTION 'Pointer requires exact latest recognition' USING ERRCODE='23514'; END IF;
 IF TG_OP='UPDATE' THEN
  SELECT * INTO previous FROM payment_recognitions WHERE id=OLD.recognition_id;
  IF NEW.order_id<>OLD.order_id OR wanted.through_observation<previous.through_observation THEN
   RAISE EXCEPTION 'Recognition pointer cannot rebind or rewind' USING ERRCODE='23514'; END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_recognition_current BEFORE INSERT OR UPDATE OR DELETE ON payment_recognition_current FOR EACH ROW EXECUTE FUNCTION guard_payment_recognition_current();

CREATE FUNCTION require_complete_payment_recognition() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE wanted payment_recognitions%ROWTYPE; latest text;
BEGIN
 IF TG_TABLE_NAME='payment_recognitions' THEN wanted=NEW;
 ELSE SELECT * INTO wanted FROM payment_recognitions WHERE id=NEW.recognition_id; END IF;
 SELECT id INTO latest FROM payment_recognitions WHERE order_id=wanted.order_id ORDER BY through_observation DESC LIMIT 1;
 IF NOT EXISTS(SELECT 1 FROM payment_recognition_current WHERE order_id=wanted.order_id AND recognition_id=latest)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=wanted.order_id
    AND e.kind='payment.recognized.v1' AND e.created_at=wanted.recorded_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',wanted.order_id,'attempt_id',wanted.attempt_id,
      'recognition_id',wanted.id,'recognition_sha256',payment_recognition_digest(wanted))) THEN
  RAISE EXCEPTION 'Recognition requires atomic current pointer and event custody' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_payment_recognition AFTER INSERT ON payment_recognitions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_recognition();
CREATE CONSTRAINT TRIGGER require_complete_payment_recognition_current AFTER INSERT OR UPDATE ON payment_recognition_current DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_recognition();

DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind) WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1','payment.recognized.v1');
CREATE UNIQUE INDEX commerce_outbox_recognition_identity ON commerce_outbox((payload->>'recognition_id')) WHERE kind='payment.recognized.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1','payment.recognized.v1'));

CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE;
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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
