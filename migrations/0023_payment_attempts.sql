-- Empty test-payment custody. No key, provider call, job dispatcher or payment state is installed.
CREATE TABLE payment_attempts (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 order_id text UNIQUE NOT NULL REFERENCES checkout_orders(id),
 basket_id text NOT NULL REFERENCES guest_baskets(id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 intent jsonb NOT NULL CHECK(jsonb_typeof(intent)='object' AND octet_length(intent::text)<=1024),
 idempotency_key text UNIQUE NOT NULL CONSTRAINT payment_attempts_key_binding CHECK(idempotency_key='piwas-test-payment-'||id),
 created_at bigint NOT NULL CHECK(created_at>=0),
 dispatch_before bigint NOT NULL CHECK(dispatch_before>created_at AND dispatch_before-created_at<=1800 AND dispatch_before<=9007199254740991),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=16384),
 configuration jsonb NOT NULL CHECK(jsonb_typeof(configuration)='object' AND octet_length(configuration::text)<=8192),
 configuration_material text NOT NULL CHECK(octet_length(configuration_material)<=8192),
 request_body text NOT NULL CHECK(octet_length(request_body)<=8192),
 UNIQUE(basket_id,effect_key)
);
CREATE FUNCTION payment_order_has_live_hold(wanted_order text, at_time bigint) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(
  SELECT 1 FROM checkout_orders o JOIN basket_hold_groups g ON g.id=o.stock_group_id
  WHERE o.id=wanted_order AND at_time>=o.created_at AND at_time<o.hold_expires_at
   AND g.owner_id=o.id AND g.created_at=o.created_at AND g.expires_at=o.hold_expires_at AND g.sealed
   AND g.members_count BETWEEN 1 AND 20
   AND g.members_count=jsonb_array_length(o.snapshot->'quote'->'lines')
   AND g.members_count=(SELECT count(*) FROM stock_holds h WHERE h.basket_group_id=g.id)
   AND NOT EXISTS(SELECT 1 FROM stock_holds h WHERE h.basket_group_id=g.id AND
    (h.state IS DISTINCT FROM 'active' OR h.owner_id IS DISTINCT FROM o.id OR h.created_at<>o.created_at OR h.expires_at<>o.hold_expires_at))
 );
$$;
CREATE FUNCTION guard_payment_attempt() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owned checkout_orders%ROWTYPE; profile jsonb; binding jsonb; gross bigint; material jsonb; request jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Payment attempts are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=NEW.order_id AND basket_id=NEW.basket_id;
 IF owned.id IS NULL OR NEW.created_at<owned.created_at OR NEW.dispatch_before<>owned.hold_expires_at
  OR NEW.intent IS DISTINCT FROM jsonb_build_object('order_id',NEW.order_id,'confirmed',true) THEN
  RAISE EXCEPTION 'Payment requires exact owned accepted order' USING ERRCODE='23514';
 END IF;
 profile=jsonb_build_object('id',NEW.configuration->'profile'->>'id','credential_fingerprint',NEW.configuration->'profile'->>'credential_fingerprint','mode','test');
 IF NEW.configuration IS DISTINCT FROM jsonb_build_object('schema_version',1,'profile',profile,'method',NEW.configuration->>'method','merchant_origin',NEW.configuration->>'merchant_origin','return_url',NEW.configuration->>'return_url','webhook_url',NEW.configuration->>'webhook_url')
  OR coalesce((profile->>'id') !~ '^pfl_.+$',true) OR octet_length(profile->>'id')>256
  OR coalesce((profile->>'credential_fingerprint') !~ '^[a-f0-9]{64}$',true)
  OR coalesce((NEW.configuration->>'method') NOT IN ('ideal','bancontact','creditcard','paypal'),true)
  OR NEW.configuration->>'merchant_origin' IS NULL OR NEW.configuration->>'return_url' IS NULL OR NEW.configuration->>'webhook_url' IS NULL THEN
  RAISE EXCEPTION 'Payment requires explicit closed test configuration' USING ERRCODE='23514';
 END IF;
 material=jsonb_build_array('piwas-mollie-test-configuration-v1',profile->>'id',profile->>'credential_fingerprint','test',NEW.configuration->>'method',NEW.configuration->>'merchant_origin',NEW.configuration->>'return_url',NEW.configuration->>'webhook_url');
 IF NEW.configuration_material::jsonb IS DISTINCT FROM material THEN
  RAISE EXCEPTION 'Payment configuration digest requires exact source material' USING ERRCODE='23514';
 END IF;
 gross=(owned.snapshot->'quote'->'total'->>'gross_minor')::bigint;
 IF gross IS NULL OR gross<=0 OR gross>9007199254740991 THEN
  RAISE EXCEPTION 'Payment requires positive bounded captured EUR' USING ERRCODE='23514';
 END IF;
 binding=jsonb_build_object('id',NEW.id,'order_id',NEW.order_id,'basket_id',NEW.basket_id,'stock_group_id',owned.stock_group_id,'profile',profile,'configuration_sha256',encode(sha256(convert_to(NEW.configuration_material,'UTF8')),'hex'),'currency','EUR','amount_minor',gross,'created_at',NEW.created_at,'dispatch_before',NEW.dispatch_before);
 IF NEW.snapshot IS DISTINCT FROM jsonb_build_object('schema_version',1,'binding',binding,'request_sha256',encode(sha256(convert_to(NEW.request_body,'UTF8')),'hex')) THEN
  RAISE EXCEPTION 'Payment snapshot requires exact immutable money, identity and digests' USING ERRCODE='23514';
 END IF;
 request=jsonb_build_object('amount',jsonb_build_object('currency','EUR','value',(gross/100)::text||'.'||lpad((gross%100)::text,2,'0')),'description','Piwas order '||owned.id,'redirectUrl',(NEW.configuration->>'return_url')||'?order_id='||owned.id||'&attempt_id='||NEW.id,'webhookUrl',NEW.configuration->>'webhook_url','method',NEW.configuration->>'method','sequenceType','oneoff','metadata',jsonb_build_object('schema_version',1,'order_id',owned.id,'attempt_id',NEW.id));
 IF NEW.request_body::jsonb IS DISTINCT FROM request THEN
  RAISE EXCEPTION 'Payment request requires exact oneoff captured intent' USING ERRCODE='23514';
 END IF;
 PERFORM id FROM stock_holds WHERE basket_group_id=owned.stock_group_id ORDER BY id FOR SHARE;
 IF NOT payment_order_has_live_hold(owned.id,NEW.created_at) THEN
  RAISE EXCEPTION 'New payment requires complete active existing reservation' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_attempt BEFORE INSERT OR UPDATE OR DELETE ON payment_attempts FOR EACH ROW EXECUTE FUNCTION guard_payment_attempt();
CREATE TABLE payment_dispatch_starts (
 attempt_id text PRIMARY KEY REFERENCES payment_attempts(id),
 id text UNIQUE NOT NULL CHECK(id ~ '^[a-f0-9]{32}$'),
 started_at bigint NOT NULL CHECK(started_at>=0 AND started_at<=9007199254740991)
);
CREATE FUNCTION guard_payment_dispatch_start() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE attempt payment_attempts%ROWTYPE; group_id text;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Payment dispatch starts are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO attempt FROM payment_attempts WHERE id=NEW.attempt_id;
 IF attempt.id IS NULL OR NEW.started_at<attempt.created_at OR NEW.started_at>=attempt.dispatch_before THEN
  RAISE EXCEPTION 'Initial dispatch requires captured live attempt' USING ERRCODE='23514';
 END IF;
 SELECT stock_group_id INTO group_id FROM checkout_orders WHERE id=attempt.order_id;
 PERFORM id FROM stock_holds WHERE basket_group_id=group_id ORDER BY id FOR SHARE;
 IF NOT payment_order_has_live_hold(attempt.order_id,NEW.started_at) THEN
  RAISE EXCEPTION 'Initial dispatch requires complete active reservation' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_dispatch_start BEFORE INSERT OR UPDATE OR DELETE ON payment_dispatch_starts FOR EACH ROW EXECUTE FUNCTION guard_payment_dispatch_start();
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1'));
CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE FUNCTION require_complete_payment_attempt() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=NEW.order_id AND e.kind='payment.attempt_prepared.v1' AND e.payload=jsonb_build_object('schema_version',1,'order_id',NEW.order_id,'attempt_id',NEW.id))
  OR NOT payment_order_has_live_hold(NEW.order_id,NEW.created_at) THEN
  RAISE EXCEPTION 'Initial payment requires prepared event and active reservation' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_payment_attempt AFTER INSERT ON payment_attempts DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_attempt();
CREATE FUNCTION require_reconcilable_payment_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_attempts a ON a.order_id=e.aggregate_id WHERE a.id=NEW.attempt_id AND e.kind='payment.reconciliation_required.v1' AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',NEW.id)) THEN
  RAISE EXCEPTION 'Dispatch requires durable reconciliation custody' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_reconcilable_payment_dispatch AFTER INSERT ON payment_dispatch_starts DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_reconcilable_payment_dispatch();
