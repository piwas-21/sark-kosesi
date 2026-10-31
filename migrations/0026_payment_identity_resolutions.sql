-- Immutable identity evidence. No order, stock or financial transition.
CREATE TABLE payment_identity_resolutions (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 revision bigint NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 through_observation bigint NOT NULL CHECK(through_observation BETWEEN 0 AND 1000),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 summary jsonb NOT NULL CHECK(jsonb_typeof(summary)='object' AND octet_length(summary::text)<=524288),
 UNIQUE(attempt_id,effect_key), UNIQUE(attempt_id,revision)
);
-- A native uniqueness conflict also aborts stale repeatable-read importers.
CREATE TABLE payment_provider_identity_claims (
 profile_id text NOT NULL CHECK(profile_id ~ '^pfl_.+$' AND octet_length(profile_id)<=256),
 provider_id text NOT NULL CHECK(provider_id ~ '^tr_.+$' AND octet_length(provider_id)<=256),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 initial_resolution_id text NOT NULL REFERENCES payment_identity_resolutions(id) DEFERRABLE INITIALLY DEFERRED,
 PRIMARY KEY(profile_id,provider_id)
);
CREATE FUNCTION guard_payment_identity_claim() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Provider identity ownership is immutable' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_identity_claim BEFORE INSERT OR UPDATE OR DELETE ON payment_provider_identity_claims FOR EACH ROW EXECUTE FUNCTION guard_payment_identity_claim();
CREATE FUNCTION require_complete_payment_identity_claim() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM payment_identity_resolutions r JOIN payment_attempts a ON a.id=r.attempt_id
   WHERE r.id=NEW.initial_resolution_id AND r.attempt_id=NEW.attempt_id
     AND a.snapshot->'binding'->'profile'->>'id'=NEW.profile_id
     AND (r.summary->'provider_ids') ? NEW.provider_id) THEN
  RAISE EXCEPTION 'Ownership requires exact original profile/attempt resolution' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_payment_identity_claim AFTER INSERT ON payment_provider_identity_claims DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_identity_claim();
CREATE FUNCTION payment_identity_summary(attempt text, through_revision bigint) RETURNS jsonb LANGUAGE sql STABLE AS $$
 SELECT jsonb_build_object('provider_ids',coalesce(jsonb_agg(DISTINCT provider_id COLLATE "C" ORDER BY provider_id COLLATE "C"),'[]'::jsonb),
 'paid_provider_ids',coalesce(jsonb_agg(DISTINCT provider_id COLLATE "C" ORDER BY provider_id COLLATE "C") FILTER(WHERE status='paid'),'[]'::jsonb))
 FROM payment_observations WHERE attempt_id=attempt AND revision<=through_revision
$$;
CREATE FUNCTION guard_payment_identity_resolution() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE started bigint; last_revision bigint; last_at bigint; observed bigint; observed_at bigint; profile text; identity text; identity_owner text;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Identity resolutions are immutable' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT started_at INTO started FROM payment_dispatch_starts WHERE attempt_id=NEW.attempt_id;
 SELECT revision,recorded_at INTO last_revision,last_at FROM payment_identity_resolutions WHERE attempt_id=NEW.attempt_id ORDER BY revision DESC LIMIT 1;
 SELECT coalesce(max(revision),0),coalesce(max(recorded_at),0) INTO observed,observed_at FROM payment_observations WHERE attempt_id=NEW.attempt_id;
 IF started IS NULL OR NEW.recorded_at<started OR NEW.recorded_at<coalesce(last_at,0)
  OR NEW.recorded_at<observed_at OR NEW.revision<>coalesce(last_revision,0)+1
  OR NEW.through_observation<>observed OR NEW.summary IS DISTINCT FROM payment_identity_summary(NEW.attempt_id,observed) THEN
  RAISE EXCEPTION 'Resolution requires complete original observation evidence' USING ERRCODE='23514';
 END IF;
 -- Native unique claims prevent one mutable provider metadata field funding two orders.
 SELECT snapshot->'binding'->'profile'->>'id' INTO profile FROM payment_attempts WHERE id=NEW.attempt_id;
 FOR identity IN SELECT value FROM jsonb_array_elements_text(NEW.summary->'provider_ids') ORDER BY value COLLATE "C" LOOP
  INSERT INTO payment_provider_identity_claims(profile_id,provider_id,attempt_id,initial_resolution_id)
   VALUES(profile,identity,NEW.attempt_id,NEW.id) ON CONFLICT(profile_id,provider_id) DO NOTHING;
  SELECT attempt_id INTO identity_owner FROM payment_provider_identity_claims WHERE profile_id=profile AND provider_id=identity;
  IF identity_owner IS DISTINCT FROM NEW.attempt_id THEN
   RAISE EXCEPTION 'Provider identity belongs to a different original attempt' USING ERRCODE='23514';
  END IF;
 END LOOP;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_identity_resolution BEFORE INSERT OR UPDATE OR DELETE ON payment_identity_resolutions FOR EACH ROW EXECUTE FUNCTION guard_payment_identity_resolution();
DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind) WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1');
CREATE UNIQUE INDEX commerce_outbox_resolution_identity ON commerce_outbox((payload->>'resolution_id')) WHERE kind='payment.identity_resolved.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1'));
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
 ELSIF NEW.kind='payment.search_page_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_search_pages p JOIN payment_attempts a ON a.id=p.attempt_id WHERE a.order_id=NEW.aggregate_id AND p.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'page_id',p.id)) THEN
   RAISE EXCEPTION 'Search page event requires exact discovery binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.identity_resolved.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_identity_resolutions r JOIN payment_attempts a ON a.id=r.attempt_id WHERE a.order_id=NEW.aggregate_id AND r.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'resolution_id',r.id)) THEN
   RAISE EXCEPTION 'Resolution event requires exact evidence binding' USING ERRCODE='23514';
  END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE FUNCTION require_complete_payment_identity_resolution() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_attempts a ON a.order_id=e.aggregate_id WHERE a.id=NEW.attempt_id AND e.kind='payment.identity_resolved.v1' AND e.created_at=NEW.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'resolution_id',NEW.id)) THEN
  RAISE EXCEPTION 'Resolution requires durable processing custody' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_payment_identity_resolution AFTER INSERT ON payment_identity_resolutions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_identity_resolution();
