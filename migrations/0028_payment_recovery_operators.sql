-- Empty finance authority and immutable bounded GET-only recovery decisions.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CHECK(capability IN
 ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review','promotion.manage','promotion.review','payment.reconcile'));
ALTER TABLE payment_reconciliation_jobs ADD COLUMN resume_revision integer NOT NULL DEFAULT 0 CHECK(resume_revision BETWEEN 0 AND 3);
CREATE TABLE payment_recovery_resumes (
 attempt_id text NOT NULL REFERENCES payment_reconciliation_jobs(attempt_id),
 revision integer NOT NULL CHECK(revision BETWEEN 1 AND 3),
 effect_key text UNIQUE NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 actor text NOT NULL REFERENCES staff_accounts(id),
 reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 500),
 profile_id text NOT NULL CHECK(length(profile_id) BETWEEN 5 AND 128),
 before_state jsonb NOT NULL CHECK(jsonb_typeof(before_state)='object' AND octet_length(before_state::text)<=4096),
 after_state jsonb NOT NULL CHECK(jsonb_typeof(after_state)='object' AND octet_length(after_state::text)<=4096),
 PRIMARY KEY(attempt_id,revision)
);
CREATE FUNCTION guard_payment_recovery_resume() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE j payment_reconciliation_jobs; expected jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Recovery decisions are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO j FROM payment_reconciliation_jobs WHERE attempt_id=NEW.attempt_id FOR UPDATE;
 expected := to_jsonb(j) || jsonb_build_object('phase',CASE WHEN j.page_id IS NULL THEN 'search' ELSE 'candidates' END,
  'failures',0,'review_reason',NULL,'resume_revision',NEW.revision,'updated_at',NEW.after_state->'updated_at','due_at',NEW.after_state->'updated_at');
 IF j.phase<>'needs_review' OR j.review_reason<>'retry_exhausted' OR j.failures<>5 OR j.polls>=10 OR j.generation>=1500
  OR NEW.before_state IS DISTINCT FROM to_jsonb(j) OR NEW.after_state IS DISTINCT FROM expected
  OR (NEW.after_state->>'updated_at')::bigint<j.updated_at
  OR (NEW.after_state->>'updated_at')::bigint>9007199254740991
  OR NEW.revision<>j.resume_revision+1
  OR NEW.revision<>(SELECT coalesce(max(revision),0)+1 FROM payment_recovery_resumes WHERE attempt_id=NEW.attempt_id)
  OR NOT EXISTS(SELECT 1 FROM payment_attempts a WHERE a.id=j.attempt_id AND a.snapshot->'binding'->'profile'->>'id'=NEW.profile_id) THEN
  RAISE EXCEPTION 'Recovery requires an exact bounded transient-failure decision' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_recovery_resume BEFORE INSERT OR UPDATE OR DELETE ON payment_recovery_resumes
FOR EACH ROW EXECUTE FUNCTION guard_payment_recovery_resume();
CREATE FUNCTION verify_payment_recovery_resume() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM payment_reconciliation_jobs j WHERE j.attempt_id=NEW.attempt_id AND to_jsonb(j)=NEW.after_state) THEN
  RAISE EXCEPTION 'Recovery decision and checkpoint must commit together' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER verify_payment_recovery_resume AFTER INSERT ON payment_recovery_resumes
DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_payment_recovery_resume();
CREATE OR REPLACE FUNCTION guard_payment_reconciliation_job() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Recovery jobs retain their custody' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_dispatch_starts s ON s.attempt_id=NEW.attempt_id JOIN payment_attempts a ON a.id=s.attempt_id
   WHERE e.id=NEW.source_event_id AND e.kind='payment.reconciliation_required.v1' AND e.aggregate_id=a.order_id
    AND e.created_at=s.started_at AND NEW.created_at=s.started_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id)) THEN
  RAISE EXCEPTION 'Recovery requires original atomic dispatch event' USING ERRCODE='23514';
 END IF;
 IF TG_OP='INSERT' AND (NEW.phase<>'search' OR NEW.generation<>0 OR NEW.lease_until IS NOT NULL OR NEW.page_id IS NOT NULL
   OR NEW.resume_revision<>0 OR NEW.candidate_index<>0 OR NEW.operation<>0 OR NEW.polls<>0 OR NEW.failures<>0 OR NEW.review_reason IS NOT NULL OR NEW.due_at<>NEW.updated_at) THEN
  RAISE EXCEPTION 'Recovery starts as explicit pending discovery' USING ERRCODE='23514';
 END IF;
 IF NEW.page_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM payment_search_pages p WHERE p.id=NEW.page_id AND p.attempt_id=NEW.attempt_id) THEN
  RAISE EXCEPTION 'Recovery page belongs to another attempt' USING ERRCODE='23514';
 END IF;
 IF TG_OP='UPDATE' AND (NEW.attempt_id<>OLD.attempt_id OR NEW.source_event_id<>OLD.source_event_id OR NEW.created_at<>OLD.created_at
   OR NEW.updated_at<OLD.updated_at OR NEW.generation<OLD.generation OR NEW.generation>OLD.generation+1
   OR NEW.operation<OLD.operation OR NEW.polls<OLD.polls
   OR NEW.resume_revision<OLD.resume_revision OR NEW.resume_revision>OLD.resume_revision+1
   OR (NEW.resume_revision<>OLD.resume_revision AND OLD.phase<>'needs_review')
   OR (OLD.phase='needs_review' AND NOT EXISTS(
    SELECT 1 FROM payment_recovery_resumes r WHERE r.attempt_id=OLD.attempt_id AND r.revision=NEW.resume_revision AND NEW.resume_revision=OLD.resume_revision+1 AND r.before_state=to_jsonb(OLD) AND r.after_state=to_jsonb(NEW)))) THEN
  RAISE EXCEPTION 'Recovery history cannot be rebound, rewound or silently restarted' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
