-- Minimal authenticated-fetch facts only; no payment/stock/fulfilment state is installed.
CREATE TABLE payment_observations (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 revision bigint NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 provider_id text NOT NULL CHECK(provider_id ~ '^tr_.+$' AND octet_length(provider_id)<=256 AND btrim(provider_id)=provider_id AND provider_id !~ '[[:cntrl:]]'),
 credential_fingerprint text NOT NULL CHECK(credential_fingerprint ~ '^[a-f0-9]{64}$'),
 status text NOT NULL CHECK(status IN ('open','pending','authorized','paid','canceled','expired','failed')),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=8192),
 UNIQUE(attempt_id,effect_key),
 UNIQUE(attempt_id,revision)
);
CREATE INDEX payment_observations_provider ON payment_observations(provider_id,attempt_id);
CREATE FUNCTION guard_payment_observation() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE attempt payment_attempts%ROWTYPE; started bigint; previous bigint; previous_at bigint; payment jsonb; expected jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Payment observations are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO attempt FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT started_at INTO started FROM payment_dispatch_starts WHERE attempt_id=NEW.attempt_id;
 IF attempt.id IS NULL OR started IS NULL OR NEW.recorded_at<started THEN
  RAISE EXCEPTION 'Observation requires original dispatch fact' USING ERRCODE='23514';
 END IF;
 SELECT revision,recorded_at INTO previous,previous_at FROM payment_observations WHERE attempt_id=NEW.attempt_id ORDER BY revision DESC LIMIT 1;
 IF NEW.revision<>coalesce(previous,0)+1 OR NEW.recorded_at<coalesce(previous_at,0) THEN
  RAISE EXCEPTION 'Observation history must be contiguous and monotonic' USING ERRCODE='23514';
 END IF;
 payment=jsonb_build_object('resource','payment','id',NEW.provider_id,'profile_id',attempt.snapshot->'binding'->'profile'->>'id','mode','test','currency','EUR','amount_minor',attempt.snapshot->'binding'->'amount_minor','metadata_schema_version',1,'order_id',attempt.order_id,'attempt_id',attempt.id,'status',NEW.status);
 expected=jsonb_build_object('schema_version',1,'id',NEW.id,'attempt_id',attempt.id,'effect_key',NEW.effect_key,'revision',NEW.revision,'recorded_at',NEW.recorded_at,'intent',jsonb_build_object('payment',payment,'credential_fingerprint',NEW.credential_fingerprint));
 IF NEW.snapshot IS DISTINCT FROM expected THEN
  RAISE EXCEPTION 'Observation requires exact minimal original-attempt binding' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_observation BEFORE INSERT OR UPDATE OR DELETE ON payment_observations FOR EACH ROW EXECUTE FUNCTION guard_payment_observation();
-- Preserve original per-order uniqueness; each new fact needs its own outbox identity.
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_aggregate_id_kind_key;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind) WHERE kind<>'payment.observation_recorded.v1';
CREATE UNIQUE INDEX commerce_outbox_observation_identity ON commerce_outbox((payload->>'observation_id')) WHERE kind='payment.observation_recorded.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1'));
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
 ELSIF NEW.kind='payment.observation_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_observations o JOIN payment_attempts a ON a.id=o.attempt_id WHERE a.order_id=NEW.aggregate_id AND o.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'observation_id',o.id)) THEN
   RAISE EXCEPTION 'Observation event requires exact historical fact binding' USING ERRCODE='23514';
  END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE FUNCTION require_complete_payment_observation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_attempts a ON a.order_id=e.aggregate_id WHERE a.id=NEW.attempt_id AND e.kind='payment.observation_recorded.v1' AND e.created_at=NEW.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'observation_id',NEW.id)) THEN
  RAISE EXCEPTION 'Observation requires durable processing custody' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_payment_observation AFTER INSERT ON payment_observations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_observation();
