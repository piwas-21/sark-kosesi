-- Callback aggregates exist independently of an order: authenticated lookup may
-- discover unknown metadata. Existing order outbox constraints remain unchanged.
CREATE TABLE callback_lookup_operations (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 profile_id text NOT NULL CHECK(profile_id ~ '^pfl_.+$' AND octet_length(profile_id)<=256 AND btrim(profile_id)=profile_id AND profile_id !~ '[[:cntrl:]]'),
 provider_id text NOT NULL CHECK(provider_id ~ '^tr_.+$' AND octet_length(provider_id)<=256 AND btrim(provider_id)=provider_id AND provider_id !~ '[[:cntrl:]]'),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991)
);
CREATE TABLE callback_lookup_fetch_starts (
 operation_id text PRIMARY KEY REFERENCES callback_lookup_operations(id),
 started_at bigint NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740991)
);
CREATE FUNCTION guard_callback_lookup_start() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback lookup starts are immutable' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM callback_lookup_operations o WHERE o.id=NEW.operation_id AND o.created_at<=NEW.started_at FOR UPDATE)
 THEN RAISE EXCEPTION 'Callback start requires original operation' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_lookup_start BEFORE INSERT OR UPDATE OR DELETE ON callback_lookup_fetch_starts FOR EACH ROW EXECUTE FUNCTION guard_callback_lookup_start();
CREATE TABLE callback_lookup_receipts (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 operation_id text NOT NULL UNIQUE REFERENCES callback_lookup_operations(id),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=8192)
);
CREATE TABLE callback_processing_events (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 operation_id text NOT NULL UNIQUE REFERENCES callback_lookup_operations(id),
 receipt_id text NOT NULL UNIQUE REFERENCES callback_lookup_receipts(id),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
 kind text NOT NULL CHECK(kind='payment.callback_lookup_recorded.v1'),
 payload jsonb NOT NULL CHECK(jsonb_typeof(payload)='object' AND octet_length(payload::text)<=512)
);
CREATE FUNCTION guard_callback_lookup_operation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback lookup operations are immutable' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_lookup_operation BEFORE UPDATE OR DELETE ON callback_lookup_operations FOR EACH ROW EXECUTE FUNCTION guard_callback_lookup_operation();
CREATE FUNCTION guard_callback_lookup_receipt() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE operation callback_lookup_operations%ROWTYPE; started bigint; payment jsonb; expected jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback lookup receipts are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO operation FROM callback_lookup_operations WHERE id=NEW.operation_id FOR UPDATE;
 SELECT started_at INTO started FROM callback_lookup_fetch_starts WHERE operation_id=NEW.operation_id;
 payment=NEW.snapshot->'payment';
 IF operation.id IS NULL OR started IS NULL OR NEW.recorded_at<started OR jsonb_typeof(payment)<>'object'
  OR payment->>'resource' IS DISTINCT FROM 'payment'
  OR payment->>'id' IS DISTINCT FROM operation.provider_id
  OR payment->>'profile_id' IS DISTINCT FROM operation.profile_id
  OR payment->>'mode' IS DISTINCT FROM 'test' OR payment->>'currency' IS DISTINCT FROM 'EUR'
  OR payment->'metadata_schema_version' IS DISTINCT FROM '1'::jsonb
  OR (payment->>'order_id') IS NULL OR (payment->>'order_id') !~ '^[a-f0-9]{32}$'
  OR (payment->>'attempt_id') IS NULL OR (payment->>'attempt_id') !~ '^[a-f0-9]{32}$'
  OR jsonb_typeof(payment->'amount_minor') IS DISTINCT FROM 'number'
  OR (payment->>'amount_minor') !~ '^[0-9]+$'
  OR (payment->>'amount_minor')::numeric NOT BETWEEN 1 AND 9007199254740991
  OR (payment->>'status') IS NULL OR (payment->>'status') NOT IN ('open','pending','authorized','paid','canceled','expired','failed')
  OR (NEW.snapshot->>'credential_fingerprint') IS NULL OR (NEW.snapshot->>'credential_fingerprint') !~ '^[a-f0-9]{64}$'
 THEN RAISE EXCEPTION 'Callback receipt requires exact bounded authenticated projection' USING ERRCODE='23514'; END IF;
 payment=jsonb_build_object('resource','payment','id',operation.provider_id,'profile_id',operation.profile_id,
  'mode','test','currency','EUR','amount_minor',payment->'amount_minor','metadata_schema_version',1,
  'order_id',payment->>'order_id','attempt_id',payment->>'attempt_id','status',payment->>'status');
 expected=jsonb_build_object('schema_version',1,'id',NEW.id,'operation',jsonb_build_object('schema_version',1,'id',operation.id,'profile_id',operation.profile_id,'provider_id',operation.provider_id,'created_at',operation.created_at),
  'started_at',started,'recorded_at',NEW.recorded_at,'payment',payment,'credential_fingerprint',NEW.snapshot->>'credential_fingerprint');
 IF NEW.snapshot IS DISTINCT FROM expected THEN RAISE EXCEPTION 'Callback receipt must be closed and exact' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_lookup_receipt BEFORE INSERT OR UPDATE OR DELETE ON callback_lookup_receipts FOR EACH ROW EXECUTE FUNCTION guard_callback_lookup_receipt();
CREATE FUNCTION guard_callback_processing_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback processing events are immutable' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM callback_lookup_receipts r WHERE r.id=NEW.receipt_id AND r.operation_id=NEW.operation_id
  AND r.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,'snapshot_sha256',encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex')))
 THEN RAISE EXCEPTION 'Callback event requires exact receipt custody' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_processing_event BEFORE INSERT OR UPDATE OR DELETE ON callback_processing_events FOR EACH ROW EXECUTE FUNCTION guard_callback_processing_event();
CREATE FUNCTION require_complete_callback_lookup() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM callback_processing_events e WHERE e.receipt_id=NEW.id AND e.operation_id=NEW.operation_id
  AND e.created_at=NEW.recorded_at AND e.kind='payment.callback_lookup_recorded.v1'
  AND e.payload=jsonb_build_object('schema_version',1,'operation_id',NEW.operation_id,'receipt_id',NEW.id,'snapshot_sha256',encode(sha256(convert_to(NEW.snapshot::text,'UTF8')),'hex')))
 THEN RAISE EXCEPTION 'Callback receipt requires durable processing custody' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_callback_lookup AFTER INSERT ON callback_lookup_receipts DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_callback_lookup();
