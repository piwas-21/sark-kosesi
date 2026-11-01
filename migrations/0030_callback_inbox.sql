-- Private bounded callback custody. No public receiver or financial transition.
CREATE TABLE callback_intakes (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 profile_id text NOT NULL CHECK(profile_id ~ '^pfl_.+$' AND octet_length(profile_id)<=256 AND btrim(profile_id)=profile_id AND profile_id !~ '[[:cntrl:]]'),
 provider_id text NOT NULL CHECK(provider_id ~ '^tr_.+$' AND octet_length(provider_id)<=256 AND btrim(provider_id)=provider_id AND provider_id !~ '[[:cntrl:]]'),
 received_at bigint NOT NULL CHECK(received_at BETWEEN 0 AND 9007199254740991)
);
CREATE INDEX callback_intakes_received ON callback_intakes(received_at DESC);
CREATE INDEX callback_intakes_profile_received ON callback_intakes(profile_id,received_at DESC);
CREATE TABLE callback_admission_quota (
 scope text PRIMARY KEY CHECK(scope='global' OR (scope ~ '^pfl_.+$' AND octet_length(scope)<=256 AND btrim(scope)=scope AND scope !~ '[[:cntrl:]]')),
 rate_window bigint NOT NULL CHECK(rate_window BETWEEN 0 AND 150119987579016),
 admitted integer NOT NULL CHECK(admitted BETWEEN 0 AND 120 AND (scope='global' OR admitted<=30)),
 last_at bigint NOT NULL CHECK(last_at BETWEEN 0 AND 9007199254740991),
 CHECK(rate_window=last_at/60)
);
INSERT INTO callback_admission_quota VALUES('global',0,0,0);
CREATE TABLE callback_intake_events (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 intake_id text NOT NULL UNIQUE REFERENCES callback_intakes(id),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
 kind text NOT NULL CHECK(kind='payment.callback_received.v1'),
 payload jsonb NOT NULL CHECK(jsonb_typeof(payload)='object' AND octet_length(payload::text)<=2048)
);
CREATE TABLE callback_inbox_jobs (
 intake_id text PRIMARY KEY REFERENCES callback_intakes(id),
 source_event_id text NOT NULL UNIQUE REFERENCES callback_intake_events(id),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
 updated_at bigint NOT NULL CHECK(updated_at BETWEEN created_at AND 9007199254740991),
 due_at bigint NOT NULL CHECK(due_at BETWEEN created_at AND 9007199254740991),
 generation bigint NOT NULL DEFAULT 0 CHECK(generation BETWEEN 0 AND 5),
 lease_until bigint CHECK(lease_until BETWEEN 0 AND 9007199254740991),
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','leased','done','needs_review')),
 review_reason text CHECK(review_reason IN ('retry_exhausted','custody_invalid','unmatched','generation_limit','paid_observed','ambiguous')),
 settled boolean NOT NULL DEFAULT false,
 CHECK((state='leased')=(lease_until IS NOT NULL)),
 CHECK(lease_until IS NULL OR (generation>0 AND lease_until=updated_at+60)),
 CHECK((state='needs_review')=(review_reason IS NOT NULL)),
 CHECK(state<>'pending' OR (generation<5 AND due_at>=updated_at)),
 CHECK(generation<>0 OR (state='pending' AND updated_at=created_at AND due_at=created_at)),
 CHECK(NOT settled OR state='done'),
 CHECK(state NOT IN ('done','needs_review') OR due_at=updated_at),
 CHECK(review_reason NOT IN ('retry_exhausted','generation_limit') OR generation=5)
);
CREATE INDEX callback_inbox_due ON callback_inbox_jobs(due_at,intake_id) WHERE state IN ('pending','leased');
CREATE TABLE callback_inbox_generations (
 intake_id text NOT NULL REFERENCES callback_inbox_jobs(intake_id),
 generation bigint NOT NULL CHECK(generation BETWEEN 1 AND 5),
 operation_id text NOT NULL UNIQUE REFERENCES callback_lookup_operations(id),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740931),
 lease_until bigint NOT NULL CHECK(lease_until=created_at+60),
 PRIMARY KEY(intake_id,generation)
);
-- Independent bounded active custody survives a damaged mutable job flag.
CREATE TABLE callback_inbox_active (
 intake_id text PRIMARY KEY REFERENCES callback_inbox_jobs(intake_id),
 profile_id text NOT NULL CHECK(profile_id ~ '^pfl_.+$' AND octet_length(profile_id)<=256),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991)
);
CREATE INDEX callback_inbox_active_profile ON callback_inbox_active(profile_id,intake_id);
-- Each immutable generation may be consumed by one leased checkpoint only.
CREATE TABLE callback_generation_claims (
 intake_id text NOT NULL,
 generation bigint NOT NULL,
 operation_id text NOT NULL UNIQUE REFERENCES callback_lookup_operations(id),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740931),
 lease_until bigint NOT NULL CHECK(lease_until=created_at+60),
 PRIMARY KEY(intake_id,generation),
 FOREIGN KEY(intake_id,generation) REFERENCES callback_inbox_generations(intake_id,generation)
);
CREATE TABLE callback_receipt_processing (
 operation_id text PRIMARY KEY REFERENCES callback_lookup_operations(id),
 event_id text UNIQUE REFERENCES callback_processing_events(id),
 processed_at bigint NOT NULL CHECK(processed_at BETWEEN 0 AND 9007199254740991),
 outcome text NOT NULL CHECK(outcome IN ('observed','paid_observed','ambiguous','unmatched','custody_invalid')),
 observation_id text UNIQUE REFERENCES payment_observations(id),
 resolution_id text UNIQUE REFERENCES payment_identity_resolutions(id),
 CHECK(outcome='custody_invalid' OR event_id IS NOT NULL),
 CHECK((outcome IN ('observed','paid_observed','ambiguous'))=(observation_id IS NOT NULL AND resolution_id IS NOT NULL)),
 CHECK(outcome IN ('observed','paid_observed','ambiguous') OR (observation_id IS NULL AND resolution_id IS NULL))
);

-- A redacted source-only stop never rewrites a previously immutable disposition.
CREATE TABLE callback_processing_quarantine (
 operation_id text PRIMARY KEY REFERENCES callback_inbox_generations(operation_id),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991)
);

CREATE FUNCTION callback_intake_event_exact(event callback_intake_events) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM callback_intakes i WHERE i.id=event.intake_id AND event.created_at=i.received_at
  AND event.kind='payment.callback_received.v1' AND event.payload=jsonb_build_object('schema_version',1,'intake_id',i.id,'profile_id',i.profile_id,'provider_id',i.provider_id,'received_at',i.received_at))
$$;
CREATE FUNCTION callback_generation_source(operation text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM callback_inbox_generations g
  JOIN callback_intakes i ON i.id=g.intake_id JOIN callback_inbox_jobs j ON j.intake_id=i.id
  JOIN callback_intake_events e ON e.id=j.source_event_id
  JOIN callback_generation_claims c ON c.intake_id=g.intake_id AND c.generation=g.generation AND c.operation_id=g.operation_id AND c.created_at=g.created_at AND c.lease_until=g.lease_until
  JOIN callback_lookup_operations o ON o.id=g.operation_id JOIN callback_lookup_fetch_starts s ON s.operation_id=o.id
  WHERE g.operation_id=operation AND g.generation BETWEEN 1 AND j.generation AND j.generation<=5
  AND callback_intake_event_exact(e) AND e.intake_id=i.id AND j.created_at=i.received_at
  AND o.profile_id=i.profile_id AND o.provider_id=i.provider_id AND o.created_at=g.created_at
  AND g.created_at>=i.received_at AND s.started_at=g.created_at AND g.lease_until=g.created_at+60)
$$;
CREATE FUNCTION callback_quarantine_exact(stopped callback_processing_quarantine) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT callback_generation_source(stopped.operation_id)
  AND EXISTS(SELECT 1 FROM callback_lookup_receipts r WHERE r.operation_id=stopped.operation_id)
  AND EXISTS(SELECT 1 FROM callback_receipt_processing p WHERE p.operation_id=stopped.operation_id)
  AND EXISTS(SELECT 1 FROM callback_inbox_generations g JOIN callback_inbox_jobs j ON j.intake_id=g.intake_id
   JOIN callback_inbox_active active ON active.intake_id=j.intake_id JOIN callback_intakes i ON i.id=j.intake_id
   WHERE g.operation_id=stopped.operation_id AND NOT j.settled AND stopped.recorded_at>=g.created_at
   AND active.profile_id=i.profile_id AND active.created_at=i.received_at)
$$;
CREATE FUNCTION guard_callback_quarantine() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE allowed boolean;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback processing quarantine is immutable' USING ERRCODE='23514'; END IF;
 -- Owning job first: serialize against settlement and active-custody release.
 -- The NOT settled predicate is rechecked by EPQ after an actual lock wait.
 PERFORM 1 FROM callback_inbox_jobs j JOIN callback_inbox_generations g ON g.intake_id=j.intake_id
  WHERE g.operation_id=NEW.operation_id AND NOT j.settled FOR UPDATE OF j;
 IF NOT FOUND THEN RAISE EXCEPTION 'Quarantine requires unsettled owning job' USING ERRCODE='23514'; END IF;
 -- This explicit SPI statement obtains a fresh post-lock snapshot under the
 -- VOLATILE trigger, even if the INSERT began before a competing commit.
 SELECT callback_quarantine_exact(NEW) INTO allowed;
 IF NOT allowed THEN RAISE EXCEPTION 'Quarantine requires exact existing source custody' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_quarantine BEFORE INSERT OR UPDATE OR DELETE ON callback_processing_quarantine FOR EACH ROW EXECUTE FUNCTION guard_callback_quarantine();
CREATE FUNCTION callback_processing_exact(processed callback_receipt_processing) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT callback_generation_source(processed.operation_id) AND CASE WHEN processed.outcome='custody_invalid' THEN
  processed.observation_id IS NULL AND processed.resolution_id IS NULL
  AND EXISTS(SELECT 1 FROM callback_inbox_generations g WHERE g.operation_id=processed.operation_id AND processed.processed_at>=g.created_at)
  AND EXISTS(SELECT 1 FROM callback_lookup_receipts r WHERE r.operation_id=processed.operation_id)
  AND (processed.event_id IS NULL OR EXISTS(SELECT 1 FROM callback_processing_events e WHERE e.id=processed.event_id AND e.operation_id=processed.operation_id))
 ELSE EXISTS(SELECT 1 FROM callback_lookup_receipts r JOIN callback_processing_events e ON e.receipt_id=r.id
  WHERE r.operation_id=processed.operation_id AND e.id=processed.event_id AND e.operation_id=r.operation_id
  AND e.created_at=r.recorded_at AND processed.processed_at>=r.recorded_at
  AND CASE WHEN octet_length(r.snapshot::text)<=8192 THEN
   e.kind='payment.callback_lookup_recorded.v1' AND e.payload=jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,'snapshot_sha256',encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex'))
   AND CASE WHEN processed.outcome='unmatched' THEN processed.observation_id IS NULL AND processed.resolution_id IS NULL
   ELSE EXISTS(SELECT 1 FROM payment_observations o JOIN payment_identity_resolutions v ON v.attempt_id=o.attempt_id
    JOIN payment_attempts a ON a.id=o.attempt_id
    WHERE o.id=processed.observation_id AND v.id=processed.resolution_id
    AND o.effect_key='callback-lookup-'||processed.operation_id AND v.effect_key='callback-inbox-'||processed.operation_id
    AND o.attempt_id=r.snapshot->'payment'->>'attempt_id' AND o.provider_id=r.snapshot->'payment'->>'id'
    AND o.status=r.snapshot->'payment'->>'status' AND o.credential_fingerprint=r.snapshot->>'credential_fingerprint'
    AND o.snapshot->'intent'->'payment'=r.snapshot->'payment'
    AND o.snapshot->'intent'->>'credential_fingerprint'=r.snapshot->>'credential_fingerprint'
    AND o.recorded_at>=r.recorded_at AND v.through_observation>=o.revision AND v.recorded_at>=o.recorded_at AND processed.processed_at>=v.recorded_at
    AND v.summary=payment_identity_summary(v.attempt_id,v.through_observation)
    AND CASE WHEN jsonb_typeof(v.summary->'provider_ids')='array' AND jsonb_typeof(v.summary->'paid_provider_ids')='array' THEN
      processed.outcome=CASE WHEN jsonb_array_length(v.summary->'provider_ids')>1 THEN 'ambiguous'
       WHEN jsonb_array_length(v.summary->'paid_provider_ids')>0 THEN 'paid_observed' ELSE 'observed' END ELSE false END
    AND EXISTS(SELECT 1 FROM commerce_outbox x WHERE x.aggregate_id=a.order_id AND x.kind='payment.observation_recorded.v1' AND x.created_at=o.recorded_at AND x.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'observation_id',o.id))
    AND EXISTS(SELECT 1 FROM commerce_outbox x WHERE x.aggregate_id=a.order_id AND x.kind='payment.identity_resolved.v1' AND x.created_at=v.recorded_at AND x.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'resolution_id',v.id))) END
   ELSE false END) END
$$;
CREATE FUNCTION callback_intake_settleable(intake text) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT EXISTS(SELECT 1 FROM callback_inbox_jobs j WHERE j.intake_id=intake AND j.state='done' AND j.generation BETWEEN 1 AND 5
  AND NOT EXISTS(SELECT 1 FROM callback_inbox_generations g JOIN callback_processing_quarantine q ON q.operation_id=g.operation_id WHERE g.intake_id=intake)
  AND (SELECT count(*) FROM callback_inbox_generations g WHERE g.intake_id=intake)=j.generation
  AND NOT EXISTS(SELECT 1 FROM callback_inbox_generations g LEFT JOIN callback_receipt_processing p ON p.operation_id=g.operation_id
   WHERE g.intake_id=intake AND (p.operation_id IS NULL OR p.outcome<>'observed' OR NOT callback_processing_exact(p))))
$$;

CREATE FUNCTION guard_callback_quota() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE latest bigint; counted integer;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Callback quotas retain custody' USING ERRCODE='23514'; END IF;
 IF TG_OP='INSERT' THEN
  IF NEW.rate_window<>0 OR NEW.admitted<>0 OR NEW.last_at<>0 THEN RAISE EXCEPTION 'Callback quotas start empty' USING ERRCODE='23514'; END IF;
  RETURN NEW;
 END IF;
 IF NEW.scope<>OLD.scope OR NEW.last_at<OLD.last_at OR NEW.rate_window<OLD.rate_window THEN
  RAISE EXCEPTION 'Callback quota clock cannot rewind' USING ERRCODE='23514';
 END IF;
 IF NEW.scope='global' THEN
  SELECT received_at INTO latest FROM callback_intakes ORDER BY received_at DESC LIMIT 1;
  SELECT count(*) INTO counted FROM (SELECT 1 FROM callback_intakes WHERE received_at>=NEW.rate_window*60 AND received_at<NEW.rate_window*60+60 LIMIT 121) q;
 ELSE
  SELECT received_at INTO latest FROM callback_intakes WHERE profile_id=NEW.scope ORDER BY received_at DESC LIMIT 1;
  SELECT count(*) INTO counted FROM (SELECT 1 FROM callback_intakes WHERE profile_id=NEW.scope AND received_at>=NEW.rate_window*60 AND received_at<NEW.rate_window*60+60 LIMIT 31) q;
 END IF;
 IF NEW.last_at IS DISTINCT FROM latest OR NEW.rate_window<>NEW.last_at/60 OR NEW.admitted<>counted THEN
  RAISE EXCEPTION 'Callback quota requires exact intake history' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_quota BEFORE INSERT OR UPDATE OR DELETE ON callback_admission_quota FOR EACH ROW EXECUTE FUNCTION guard_callback_quota();
CREATE FUNCTION guard_callback_intake() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback intakes are immutable' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_intake BEFORE UPDATE OR DELETE ON callback_intakes FOR EACH ROW EXECUTE FUNCTION guard_callback_intake();
CREATE FUNCTION admit_callback_quota() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE global_quota callback_admission_quota%ROWTYPE; profile_quota callback_admission_quota%ROWTYPE; selected_window bigint;
BEGIN
 SELECT * INTO global_quota FROM callback_admission_quota WHERE scope='global' FOR UPDATE;
 IF global_quota.scope IS NULL OR NEW.received_at<global_quota.last_at THEN
  RAISE EXCEPTION 'Callback admission clock unavailable' USING ERRCODE='23514';
 END IF;
 INSERT INTO callback_admission_quota(scope,rate_window,admitted,last_at) VALUES(NEW.profile_id,0,0,0) ON CONFLICT(scope) DO NOTHING;
 SELECT * INTO profile_quota FROM callback_admission_quota WHERE scope=NEW.profile_id FOR UPDATE;
 IF NEW.received_at<profile_quota.last_at THEN RAISE EXCEPTION 'Callback admission clock unavailable' USING ERRCODE='23514'; END IF;
 selected_window=NEW.received_at/60;
 IF (global_quota.rate_window=selected_window AND global_quota.admitted>=120) OR (profile_quota.rate_window=selected_window AND profile_quota.admitted>=30) THEN
  RAISE EXCEPTION 'Callback admission rate exhausted' USING ERRCODE='23514';
 END IF;
 UPDATE callback_admission_quota SET rate_window=selected_window,last_at=NEW.received_at,admitted=CASE WHEN rate_window=selected_window THEN admitted+1 ELSE 1 END WHERE scope IN ('global',NEW.profile_id);
 RETURN NULL;
END;
$$;
CREATE TRIGGER admit_callback_quota AFTER INSERT ON callback_intakes FOR EACH ROW EXECUTE FUNCTION admit_callback_quota();
CREATE FUNCTION guard_callback_intake_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' OR NOT callback_intake_event_exact(NEW) THEN
  RAISE EXCEPTION 'Callback intake event requires exact immutable intake' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_intake_event BEFORE INSERT OR UPDATE OR DELETE ON callback_intake_events FOR EACH ROW EXECUTE FUNCTION guard_callback_intake_event();
CREATE FUNCTION require_callback_intake_custody() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM callback_inbox_jobs j JOIN callback_intake_events e ON e.id=j.source_event_id
  WHERE j.intake_id=NEW.id AND j.created_at=NEW.received_at AND e.intake_id=NEW.id AND callback_intake_event_exact(e)
  AND EXISTS(SELECT 1 FROM callback_inbox_active a WHERE a.intake_id=NEW.id AND a.created_at=NEW.received_at AND a.profile_id=NEW.profile_id)) THEN
  RAISE EXCEPTION 'Callback intake requires atomic event and job' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_callback_intake_custody AFTER INSERT ON callback_intakes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_callback_intake_custody();

CREATE FUNCTION guard_callback_generation() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE intake callback_intakes%ROWTYPE; job callback_inbox_jobs%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback generation history is immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO job FROM callback_inbox_jobs WHERE intake_id=NEW.intake_id FOR UPDATE;
 SELECT * INTO intake FROM callback_intakes WHERE id=NEW.intake_id;
 IF job.intake_id IS NULL OR job.settled OR job.state NOT IN ('pending','leased') OR NEW.generation<>job.generation+1
  OR NEW.created_at<job.updated_at OR NEW.created_at<job.due_at OR (job.lease_until IS NOT NULL AND NEW.created_at<job.lease_until)
  OR EXISTS(SELECT 1 FROM callback_inbox_generations g JOIN callback_lookup_receipts r ON r.operation_id=g.operation_id WHERE g.intake_id=job.intake_id AND g.generation=job.generation)
  OR NOT EXISTS(SELECT 1 FROM callback_lookup_operations o JOIN callback_lookup_fetch_starts s ON s.operation_id=o.id WHERE o.id=NEW.operation_id AND o.profile_id=intake.profile_id AND o.provider_id=intake.provider_id AND o.created_at=NEW.created_at AND s.started_at=NEW.created_at)
 THEN RAISE EXCEPTION 'Callback generation requires exact bounded claim and GET start' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_generation BEFORE INSERT OR UPDATE OR DELETE ON callback_inbox_generations FOR EACH ROW EXECUTE FUNCTION guard_callback_generation();
CREATE FUNCTION require_callback_generation_custody() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM callback_inbox_jobs j WHERE j.intake_id=NEW.intake_id AND j.generation=NEW.generation AND j.state='leased' AND j.updated_at=NEW.created_at AND j.lease_until=NEW.lease_until
  AND EXISTS(SELECT 1 FROM callback_generation_claims c WHERE c.intake_id=NEW.intake_id AND c.generation=NEW.generation AND c.operation_id=NEW.operation_id AND c.created_at=NEW.created_at AND c.lease_until=NEW.lease_until)) THEN
  RAISE EXCEPTION 'Callback generation requires atomic leased checkpoint' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_callback_generation_custody AFTER INSERT ON callback_inbox_generations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_callback_generation_custody();
CREATE FUNCTION guard_callback_processing() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' OR NOT callback_processing_exact(NEW) THEN
  RAISE EXCEPTION 'Callback disposition requires exact independent source custody' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_processing BEFORE INSERT OR UPDATE OR DELETE ON callback_receipt_processing FOR EACH ROW EXECUTE FUNCTION guard_callback_processing();

CREATE FUNCTION guard_callback_job() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_outcome text; processed_at bigint; settlement_valid boolean;
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Callback jobs retain custody' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM callback_intakes i JOIN callback_intake_events e ON e.intake_id=i.id
  WHERE i.id=NEW.intake_id AND e.id=NEW.source_event_id AND NEW.created_at=i.received_at AND callback_intake_event_exact(e)) THEN
  RAISE EXCEPTION 'Callback job requires immutable intake event' USING ERRCODE='23514';
 END IF;
 IF TG_OP='INSERT' THEN
  IF NEW.state<>'pending' OR NEW.generation<>0 OR NEW.lease_until IS NOT NULL OR NEW.review_reason IS NOT NULL OR NEW.settled OR NEW.updated_at<>NEW.created_at OR NEW.due_at<>NEW.created_at THEN
   RAISE EXCEPTION 'Callback job starts pending without fetch authority' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
 END IF;
 IF OLD.settled OR NEW.intake_id<>OLD.intake_id OR NEW.source_event_id<>OLD.source_event_id OR NEW.created_at<>OLD.created_at OR NEW.updated_at<OLD.updated_at THEN
  RAISE EXCEPTION 'Callback job custody cannot be rebound or rewound' USING ERRCODE='23514';
 END IF;
 IF NEW.generation=OLD.generation+1 AND NEW.state='leased' AND OLD.state IN ('pending','leased') AND NOT NEW.settled
  AND NEW.due_at=OLD.due_at AND NEW.review_reason IS NULL
  AND EXISTS(SELECT 1 FROM callback_inbox_generations g WHERE g.intake_id=NEW.intake_id AND g.generation=NEW.generation AND g.created_at=NEW.updated_at AND g.lease_until=NEW.lease_until) THEN RETURN NEW; END IF;
 IF NEW.generation<>OLD.generation THEN RAISE EXCEPTION 'Callback job requires exact generation custody' USING ERRCODE='23514'; END IF;
 IF OLD.state='done' AND NEW.state='done' AND NEW.settled THEN
  -- A VOLATILE trigger's separate SPI read sees quarantines committed while
  -- this UPDATE waited for its owning job lock; do not reuse a stale IF plan.
  SELECT callback_intake_settleable(NEW.intake_id) INTO settlement_valid;
  IF NEW.lease_until IS NULL AND NEW.review_reason IS NULL AND NEW.due_at=NEW.updated_at AND settlement_valid THEN RETURN NEW; END IF;
 END IF;
 IF OLD.state<>'leased' OR NEW.settled OR NEW.lease_until IS NOT NULL OR NEW.state NOT IN ('pending','done','needs_review') THEN
  RAISE EXCEPTION 'Callback terminal checkpoint cannot silently restart' USING ERRCODE='23514';
 END IF;
 SELECT 'custody_invalid',q.recorded_at INTO current_outcome,processed_at FROM callback_inbox_generations g JOIN callback_processing_quarantine q ON q.operation_id=g.operation_id
  WHERE g.intake_id=NEW.intake_id AND g.generation=NEW.generation AND callback_quarantine_exact(q);
 IF current_outcome IS NULL THEN
  SELECT p.outcome,p.processed_at INTO current_outcome,processed_at FROM callback_inbox_generations g JOIN callback_receipt_processing p ON p.operation_id=g.operation_id
   WHERE g.intake_id=NEW.intake_id AND g.generation=NEW.generation AND callback_processing_exact(p);
 END IF;
 IF current_outcome IS NOT NULL AND NEW.updated_at>=processed_at AND NEW.due_at=NEW.updated_at AND (
  (current_outcome='observed' AND NEW.state='done' AND NEW.review_reason IS NULL)
  OR (current_outcome<>'observed' AND NEW.state='needs_review' AND NEW.review_reason=current_outcome)) THEN RETURN NEW; END IF;
 IF current_outcome IS NULL AND OLD.lease_until>NEW.updated_at THEN
  IF NEW.state='pending' AND NEW.generation<5 AND NEW.review_reason IS NULL AND NEW.due_at=NEW.updated_at+60*(1::bigint<<(NEW.generation::integer-1))
   AND NOT EXISTS(SELECT 1 FROM callback_inbox_generations g JOIN callback_lookup_receipts r ON r.operation_id=g.operation_id WHERE g.intake_id=NEW.intake_id AND g.generation=NEW.generation) THEN RETURN NEW; END IF;
  IF NEW.state='needs_review' AND NEW.due_at=NEW.updated_at AND (NEW.review_reason='custody_invalid' OR (NEW.review_reason='retry_exhausted' AND NEW.generation=5)) THEN RETURN NEW; END IF;
 END IF;
 IF current_outcome IS NULL AND NEW.state='needs_review' AND NEW.review_reason='generation_limit' AND NEW.generation=5 AND NEW.updated_at>=OLD.lease_until AND NEW.due_at=NEW.updated_at THEN RETURN NEW; END IF;
 RAISE EXCEPTION 'Callback checkpoint requires current lease or exact persisted disposition' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER guard_callback_job BEFORE INSERT OR UPDATE OR DELETE ON callback_inbox_jobs FOR EACH ROW EXECUTE FUNCTION guard_callback_job();

CREATE FUNCTION guard_callback_active() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE active_count integer; profile_count integer;
BEGIN
 IF TG_OP='UPDATE' THEN RAISE EXCEPTION 'Callback active custody cannot be rebound' USING ERRCODE='23514'; END IF;
 IF TG_OP='DELETE' THEN
  IF NOT EXISTS(SELECT 1 FROM callback_inbox_jobs j WHERE j.intake_id=OLD.intake_id AND j.settled AND callback_intake_settleable(j.intake_id)) THEN
   RAISE EXCEPTION 'Callback capacity requires complete successful settlement' USING ERRCODE='23514';
  END IF;
  RETURN OLD;
 END IF;
 PERFORM 1 FROM callback_admission_quota WHERE scope='global' FOR UPDATE;
 IF EXISTS(SELECT 1 FROM (SELECT * FROM callback_inbox_active ORDER BY intake_id LIMIT 201) a
  LEFT JOIN callback_intakes i ON i.id=a.intake_id
  LEFT JOIN callback_inbox_jobs j ON j.intake_id=a.intake_id
  LEFT JOIN callback_intake_events e ON e.id=j.source_event_id
  WHERE i.id IS NULL OR j.intake_id IS NULL OR e.id IS NULL OR j.settled
   OR a.profile_id IS DISTINCT FROM i.profile_id OR a.created_at IS DISTINCT FROM i.received_at
   OR j.created_at IS DISTINCT FROM i.received_at OR e.intake_id IS DISTINCT FROM i.id
   OR NOT callback_intake_event_exact(e)) THEN
  RAISE EXCEPTION 'Callback existing active custody is invalid' USING ERRCODE='23514';
 END IF;
 IF NOT EXISTS(SELECT 1 FROM callback_intakes i JOIN callback_inbox_jobs j ON j.intake_id=i.id JOIN callback_intake_events e ON e.id=j.source_event_id
  WHERE i.id=NEW.intake_id AND NEW.profile_id=i.profile_id AND NEW.created_at=i.received_at AND callback_intake_event_exact(e)
  AND j.state='pending' AND j.generation=0 AND NOT j.settled) THEN
  RAISE EXCEPTION 'Callback active capacity requires exact initial intake custody' USING ERRCODE='23514';
 END IF;
 SELECT count(*) INTO active_count FROM (SELECT 1 FROM callback_inbox_active LIMIT 201) q;
 SELECT count(*) INTO profile_count FROM (SELECT 1 FROM callback_inbox_active WHERE profile_id=NEW.profile_id LIMIT 51) q;
 IF active_count>=200 OR profile_count>=50 THEN RAISE EXCEPTION 'Callback admission capacity exhausted' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_active BEFORE INSERT OR UPDATE OR DELETE ON callback_inbox_active FOR EACH ROW EXECUTE FUNCTION guard_callback_active();
CREATE FUNCTION require_callback_active_custody() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_job callback_inbox_jobs%ROWTYPE;
BEGIN
 SELECT * INTO current_job FROM callback_inbox_jobs WHERE intake_id=NEW.intake_id;
 IF current_job.settled THEN
  IF EXISTS(SELECT 1 FROM callback_inbox_active a WHERE a.intake_id=NEW.intake_id) OR NOT callback_intake_settleable(NEW.intake_id) THEN
   RAISE EXCEPTION 'Callback settled checkpoint requires exact absence and source proof' USING ERRCODE='23514';
  END IF;
 ELSE
  IF NOT EXISTS(SELECT 1 FROM callback_inbox_active a JOIN callback_intakes i ON i.id=a.intake_id
   WHERE a.intake_id=NEW.intake_id AND a.profile_id=i.profile_id AND a.created_at=i.received_at) THEN
   RAISE EXCEPTION 'Callback unsettled checkpoint requires independent active custody' USING ERRCODE='23514';
  END IF;
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_callback_active_custody AFTER INSERT OR UPDATE ON callback_inbox_jobs DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_callback_active_custody();
CREATE FUNCTION guard_callback_generation_claim() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Callback consumed claims are immutable' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM callback_inbox_generations g JOIN callback_inbox_jobs j ON j.intake_id=g.intake_id
  WHERE g.intake_id=NEW.intake_id AND g.generation=NEW.generation AND g.operation_id=NEW.operation_id AND g.created_at=NEW.created_at AND g.lease_until=NEW.lease_until
  AND j.state='leased' AND j.generation=g.generation AND j.updated_at=g.created_at AND j.lease_until=g.lease_until) THEN
  RAISE EXCEPTION 'Callback consumed claim requires exact current leased checkpoint' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_callback_generation_claim BEFORE INSERT OR UPDATE OR DELETE ON callback_generation_claims FOR EACH ROW EXECUTE FUNCTION guard_callback_generation_claim();
CREATE FUNCTION consume_callback_generation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.generation=OLD.generation+1 THEN
  INSERT INTO callback_generation_claims(intake_id,generation,operation_id,created_at,lease_until)
   SELECT g.intake_id,g.generation,g.operation_id,g.created_at,g.lease_until FROM callback_inbox_generations g
   WHERE g.intake_id=NEW.intake_id AND g.generation=NEW.generation;
 END IF;
 RETURN NULL;
END;
$$;
CREATE TRIGGER consume_callback_generation AFTER UPDATE ON callback_inbox_jobs FOR EACH ROW EXECUTE FUNCTION consume_callback_generation();
