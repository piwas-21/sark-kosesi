-- Private bounded exact-item GET supervision. Observations convey no financial authority.
CREATE TABLE reversal_item_jobs (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 observation_id text NOT NULL REFERENCES payment_observations(id),
 page_id text NOT NULL REFERENCES payment_reversal_pages(id),
 ordinal integer NOT NULL CHECK(ordinal BETWEEN 1 AND 10),
 child_id text NOT NULL,
 enrollment_key text NOT NULL CHECK(enrollment_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 profile_id text NOT NULL CHECK(payment_reversal_page_provider_id(profile_id,'pfl_')),
 payment_id text NOT NULL CHECK(payment_reversal_page_provider_id(payment_id,'tr_')),
 reversal_kind text NOT NULL CHECK(reversal_kind IN ('refund','chargeback')),
 credential_fingerprint text NOT NULL CHECK(credential_fingerprint ~ '^[a-f0-9]{64}$'),
 state text NOT NULL CHECK(state IN ('pending','leased','review','observed')),
 review_reason text CHECK(review_reason IN ('uncertain','invalid','retry_exhausted','observation_limit')),
 receipt_id text REFERENCES payment_reversal_observations(id),
 generation integer NOT NULL CHECK(generation BETWEEN 0 AND 3),
 failures integer NOT NULL CHECK(failures BETWEEN 0 AND 3),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
 updated_at bigint NOT NULL CHECK(updated_at BETWEEN created_at AND 9007199254740991),
 due_at bigint NOT NULL CHECK(due_at BETWEEN created_at AND 9007199254740991),
 lease_until bigint CHECK(lease_until BETWEEN 0 AND 9007199254740991),
 UNIQUE(attempt_id,enrollment_key), UNIQUE(page_id,ordinal),
 CHECK(payment_reversal_page_provider_id(child_id,CASE reversal_kind WHEN 'refund' THEN 're_' ELSE 'chb_' END)),
 CHECK((state='review')=(review_reason IS NOT NULL)),
 CHECK((state='observed')=(receipt_id IS NOT NULL)),
 CHECK((state='leased')=(lease_until IS NOT NULL)),
 CHECK(state<>'leased' OR (generation>0 AND lease_until=updated_at+60)),
 CHECK(state<>'pending' OR (generation<3 AND failures<3 AND due_at>=updated_at)),
 CHECK(state NOT IN ('review','observed') OR due_at=updated_at)
);
CREATE INDEX reversal_item_due ON reversal_item_jobs(due_at,id) WHERE state IN ('pending','leased');

CREATE TABLE reversal_item_operations (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 job_id text NOT NULL REFERENCES reversal_item_jobs(id),
 generation integer NOT NULL CHECK(generation BETWEEN 1 AND 3),
 started_at bigint NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740931),
 lease_until bigint NOT NULL CHECK(lease_until=started_at+60),
 outcome text CHECK(outcome IN ('observed','transient','invalid','uncertain','capped')),
 finished_at bigint CHECK(finished_at BETWEEN started_at AND 9007199254740991),
 receipt_id text UNIQUE REFERENCES payment_reversal_observations(id),
 start_xid xid8 NOT NULL DEFAULT pg_current_xact_id(),
 finish_xid xid8,
 UNIQUE(job_id,generation),
 CHECK((outcome IS NULL)=(finished_at IS NULL)),
 CHECK((outcome IS NULL)=(finish_xid IS NULL)),
 CHECK((outcome IS NOT DISTINCT FROM 'observed')=(receipt_id IS NOT NULL))
);

CREATE FUNCTION guard_reversal_item_job() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE attempt payment_attempts%ROWTYPE; observation payment_observations%ROWTYPE;
 op reversal_item_operations%ROWTYPE; page payment_reversal_pages%ROWTYPE;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Reversal item jobs retain custody' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Reversal item requires read committed' USING ERRCODE='23514'; END IF;
 SELECT * INTO attempt FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT * INTO observation FROM payment_observations WHERE id=NEW.observation_id;
 IF attempt.id IS NULL OR observation.id IS NULL OR observation.attempt_id IS DISTINCT FROM NEW.attempt_id
  OR observation.status IS DISTINCT FROM 'paid' OR observation.provider_id IS DISTINCT FROM NEW.payment_id
  OR observation.snapshot IS DISTINCT FROM jsonb_build_object('schema_version',1,'id',observation.id,
   'attempt_id',attempt.id,'effect_key',observation.effect_key,'revision',observation.revision,'recorded_at',observation.recorded_at,
   'intent',jsonb_build_object('payment',jsonb_build_object('resource','payment','id',observation.provider_id,
    'profile_id',NEW.profile_id,'mode','test','currency','EUR','amount_minor',attempt.snapshot->'binding'->'amount_minor',
    'metadata_schema_version',1,'order_id',attempt.order_id,'attempt_id',attempt.id,'status','paid'),
    'credential_fingerprint',observation.credential_fingerprint))
  OR attempt.snapshot->'binding'->'profile'->>'id' IS DISTINCT FROM NEW.profile_id
  OR attempt.snapshot->'binding'->'profile'->>'mode' IS DISTINCT FROM 'test'
  OR attempt.snapshot->'binding'->>'currency' IS DISTINCT FROM 'EUR'
  OR attempt.configuration->'profile' IS DISTINCT FROM attempt.snapshot->'binding'->'profile'
  OR attempt.snapshot->'binding'->>'id' IS DISTINCT FROM attempt.id
  OR attempt.snapshot->'binding'->>'order_id' IS DISTINCT FROM attempt.order_id
  OR attempt.snapshot->'binding'->>'basket_id' IS DISTINCT FROM attempt.basket_id
  OR NOT EXISTS(SELECT 1 FROM checkout_orders o WHERE o.id=attempt.order_id AND o.basket_id=attempt.basket_id)
  OR NOT EXISTS(SELECT 1 FROM payment_dispatch_starts s JOIN commerce_outbox e ON e.aggregate_id=attempt.order_id
   WHERE s.attempt_id=attempt.id AND s.started_at<=observation.recorded_at AND e.kind='payment.reconciliation_required.v1'
   AND e.created_at=s.started_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',attempt.order_id,
    'attempt_id',attempt.id,'dispatch_id',s.id))
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=attempt.order_id AND e.kind='payment.observation_recorded.v1'
   AND e.created_at=observation.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',attempt.order_id,'attempt_id',attempt.id,'observation_id',observation.id))
  OR NEW.created_at<observation.recorded_at THEN
  RAISE EXCEPTION 'Reversal item requires exact paid observation and dispatch' USING ERRCODE='23514'; END IF;
 SELECT * INTO page FROM payment_reversal_pages WHERE id=NEW.page_id;
 IF page.id IS NULL OR page.attempt_id IS DISTINCT FROM NEW.attempt_id
  OR page.order_id IS DISTINCT FROM attempt.order_id OR page.payment_id IS DISTINCT FROM NEW.payment_id
  OR page.reversal_kind IS DISTINCT FROM NEW.reversal_kind
  OR page.credential_fingerprint IS DISTINCT FROM NEW.credential_fingerprint
  OR page.candidates[NEW.ordinal] IS DISTINCT FROM NEW.child_id OR NEW.created_at<page.recorded_at
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=page.event_id
   AND e.aggregate_id=page.order_id AND e.kind='payment.reversal_page_recorded.v1'
   AND e.created_at=page.recorded_at AND e.payload=payment_reversal_page_event(page)) THEN
  RAISE EXCEPTION 'Reversal item requires exact page candidate and event' USING ERRCODE='23514'; END IF;
 IF TG_OP='INSERT' THEN
  IF NEW.state<>'pending' OR NEW.review_reason IS NOT NULL OR NEW.receipt_id IS NOT NULL OR NEW.generation<>0
   OR NEW.failures<>0 OR NEW.lease_until IS NOT NULL OR NEW.updated_at<>NEW.created_at OR NEW.due_at<>NEW.created_at
   OR (SELECT count(*) FROM (SELECT 1 FROM reversal_item_jobs WHERE attempt_id=NEW.attempt_id LIMIT 128) bounded)>=128 THEN
   RAISE EXCEPTION 'Reversal item requires bounded initial pending custody' USING ERRCODE='23514'; END IF;
  RETURN NEW;
 END IF;
 IF OLD.state IN ('review','observed') OR NEW.id<>OLD.id OR NEW.attempt_id<>OLD.attempt_id OR NEW.observation_id<>OLD.observation_id
  OR NEW.page_id<>OLD.page_id OR NEW.ordinal<>OLD.ordinal OR NEW.child_id<>OLD.child_id
  OR NEW.enrollment_key<>OLD.enrollment_key OR NEW.profile_id<>OLD.profile_id OR NEW.payment_id<>OLD.payment_id
  OR NEW.reversal_kind<>OLD.reversal_kind OR NEW.credential_fingerprint<>OLD.credential_fingerprint
  OR NEW.created_at<>OLD.created_at OR NEW.updated_at<OLD.updated_at THEN
  RAISE EXCEPTION 'Reversal item custody is immutable and cannot rewind' USING ERRCODE='23514'; END IF;
 IF OLD.state='pending' AND NEW.state='leased' AND NEW.generation=OLD.generation+1 THEN
  SELECT * INTO op FROM reversal_item_operations WHERE job_id=NEW.id AND generation=NEW.generation;
  IF op.id IS NOT NULL AND op.outcome IS NULL AND op.start_xid=pg_current_xact_id()
   AND op.started_at=NEW.updated_at AND op.lease_until=NEW.lease_until AND NEW.updated_at>=OLD.due_at
   AND NEW.due_at=OLD.due_at AND NEW.failures=OLD.failures AND NEW.receipt_id IS NULL THEN RETURN NEW; END IF;
 END IF;
 IF OLD.state<>'leased' OR NEW.generation<>OLD.generation OR NEW.lease_until IS NOT NULL THEN
  RAISE EXCEPTION 'Reversal item checkpoint requires its lease' USING ERRCODE='23514'; END IF;
 SELECT * INTO op FROM reversal_item_operations WHERE job_id=NEW.id AND generation=NEW.generation;
 IF op.id IS NULL OR op.outcome IS NULL OR op.finish_xid IS DISTINCT FROM pg_current_xact_id()
  OR op.finished_at<>NEW.updated_at OR op.started_at<>OLD.updated_at OR op.lease_until<>OLD.lease_until THEN
  RAISE EXCEPTION 'Reversal item checkpoint requires exact operation' USING ERRCODE='23514'; END IF;
 IF op.outcome='observed' AND NEW.state='observed' AND NEW.receipt_id=op.receipt_id
  AND NEW.failures=OLD.failures AND NEW.due_at=NEW.updated_at THEN RETURN NEW; END IF;
 IF NEW.receipt_id IS NULL THEN
  IF op.outcome='transient' AND NEW.failures=OLD.failures+1 THEN
   IF NEW.failures=3 AND NEW.state='review' AND NEW.review_reason='retry_exhausted' AND NEW.due_at=NEW.updated_at THEN RETURN NEW; END IF;
   IF NEW.failures<3 AND NEW.generation<3 AND NEW.state='pending' AND NEW.review_reason IS NULL AND NEW.due_at=NEW.updated_at+60 THEN RETURN NEW; END IF;
  ELSIF NEW.failures=OLD.failures AND NEW.state='review' AND NEW.due_at=NEW.updated_at AND
   ((op.outcome='invalid' AND NEW.review_reason='invalid') OR (op.outcome='uncertain' AND NEW.review_reason='uncertain')
    OR (op.outcome='capped' AND NEW.review_reason='observation_limit')) THEN RETURN NEW;
  END IF;
 END IF;
 RAISE EXCEPTION 'Invalid reversal item checkpoint' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER guard_reversal_item_job BEFORE INSERT OR UPDATE OR DELETE ON reversal_item_jobs FOR EACH ROW EXECUTE FUNCTION guard_reversal_item_job();

CREATE FUNCTION guard_reversal_item_operation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE job reversal_item_jobs%ROWTYPE; receipt payment_reversal_observations%ROWTYPE; observation_count bigint;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Reversal item operations retain custody' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Reversal item operation requires read committed' USING ERRCODE='23514'; END IF;
 SELECT * INTO job FROM reversal_item_jobs WHERE id=NEW.job_id;
 PERFORM 1 FROM payment_attempts WHERE id=job.attempt_id FOR UPDATE;
 SELECT * INTO job FROM reversal_item_jobs WHERE id=NEW.job_id FOR UPDATE;
 IF TG_OP='INSERT' THEN
  NEW.start_xid=pg_current_xact_id(); NEW.finish_xid=NULL;
  IF job.id IS NULL OR job.state<>'pending' OR NEW.generation<>job.generation+1
   OR NEW.started_at<job.due_at OR NEW.started_at<job.updated_at OR NEW.outcome IS NOT NULL
   OR NEW.finished_at IS NOT NULL OR NEW.receipt_id IS NOT NULL THEN
   RAISE EXCEPTION 'Reversal item operation requires pending bounded claim' USING ERRCODE='23514'; END IF;
  RETURN NEW;
 END IF;
 IF OLD.outcome IS NOT NULL OR NEW.id<>OLD.id OR NEW.job_id<>OLD.job_id OR NEW.generation<>OLD.generation
  OR NEW.started_at<>OLD.started_at OR NEW.lease_until<>OLD.lease_until
  OR NEW.start_xid IS DISTINCT FROM OLD.start_xid OR OLD.start_xid=pg_current_xact_id() OR NEW.finish_xid IS DISTINCT FROM OLD.finish_xid
  OR NEW.outcome IS NULL OR NEW.finished_at IS NULL OR job.state<>'leased' OR job.generation<>OLD.generation
  OR job.updated_at<>OLD.started_at OR job.lease_until<>OLD.lease_until THEN
  RAISE EXCEPTION 'Reversal item operation requires exact uncompleted current lease' USING ERRCODE='23514'; END IF;
 NEW.finish_xid=pg_current_xact_id();
 SELECT * INTO receipt FROM payment_reversal_observations WHERE attempt_id=job.attempt_id AND effect_key=NEW.id;
 IF NEW.outcome='observed' THEN
  IF receipt.id IS NULL OR receipt.id IS DISTINCT FROM NEW.receipt_id
   OR receipt.payment_id IS DISTINCT FROM job.payment_id OR receipt.child_id IS DISTINCT FROM job.child_id
   OR receipt.credential_fingerprint IS DISTINCT FROM job.credential_fingerprint
   OR (CASE WHEN job.reversal_kind='refund' THEN receipt.phase NOT LIKE 'refund_%' ELSE receipt.phase NOT LIKE 'chargeback_%' END)
   OR receipt.recorded_at<NEW.started_at OR NEW.finished_at<receipt.recorded_at
   OR NOT EXISTS(SELECT 1 FROM payment_attempts a WHERE a.id=job.attempt_id AND a.order_id=receipt.order_id)
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=receipt.event_id
    AND e.aggregate_id=receipt.order_id AND e.kind='payment.reversal_observed.v1' AND e.created_at=receipt.recorded_at
    AND e.payload=payment_reversal_observation_event(receipt)) THEN
   RAISE EXCEPTION 'Reversal item operation requires exact observation receipt' USING ERRCODE='23514'; END IF;
 ELSE
  IF receipt.id IS NOT NULL OR NEW.receipt_id IS NOT NULL THEN
   RAISE EXCEPTION 'Reversal item operation cannot contradict a committed observation' USING ERRCODE='23514'; END IF;
  IF NEW.outcome='uncertain' THEN
   IF NEW.finished_at<NEW.lease_until THEN RAISE EXCEPTION 'Uncertain reversal item requires expired lease' USING ERRCODE='23514'; END IF;
  ELSIF NEW.finished_at>=NEW.lease_until THEN
   RAISE EXCEPTION 'Reversal item operation requires live lease' USING ERRCODE='23514';
  END IF;
  IF NEW.outcome='capped' THEN
   SELECT count(*) INTO observation_count FROM (SELECT 1 FROM payment_reversal_observations WHERE attempt_id=job.attempt_id LIMIT 128) bounded;
   IF observation_count<128 THEN RAISE EXCEPTION 'Reversal item cap requires exhausted observation history' USING ERRCODE='23514'; END IF;
  END IF;
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_reversal_item_operation BEFORE INSERT OR UPDATE OR DELETE ON reversal_item_operations FOR EACH ROW EXECUTE FUNCTION guard_reversal_item_operation();

-- A durable start and each settlement must be paired with its exact checkpoint.
CREATE FUNCTION require_reversal_item_operation_checkpoint() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE job reversal_item_jobs%ROWTYPE; op reversal_item_operations%ROWTYPE;
BEGIN
 SELECT * INTO job FROM reversal_item_jobs WHERE id=NEW.job_id;
 SELECT * INTO op FROM reversal_item_operations WHERE id=NEW.id;
 IF job.id IS NULL OR op.id IS NULL OR job.generation<>op.generation OR
  (op.outcome IS NULL AND (job.state<>'leased' OR job.updated_at<>op.started_at OR job.lease_until IS DISTINCT FROM op.lease_until)) OR
  (op.outcome IS NOT NULL AND (job.state='leased' OR job.updated_at<>op.finished_at)) THEN
  RAISE EXCEPTION 'Reversal item operation requires atomic job checkpoint' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_reversal_item_operation_checkpoint AFTER INSERT OR UPDATE ON reversal_item_operations
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_reversal_item_operation_checkpoint();
