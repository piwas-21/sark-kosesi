-- Private supervised test-only recovery; no POST dispatch or financial transition.
CREATE TABLE payment_reconciliation_jobs (
 attempt_id text PRIMARY KEY REFERENCES payment_dispatch_starts(attempt_id),
 source_event_id text UNIQUE NOT NULL REFERENCES commerce_outbox(id),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
 updated_at bigint NOT NULL CHECK(updated_at BETWEEN created_at AND 9007199254740991),
 due_at bigint NOT NULL CHECK(due_at BETWEEN created_at AND 9007199254740991),
 generation bigint NOT NULL DEFAULT 0 CHECK(generation BETWEEN 0 AND 1500),
 lease_until bigint CHECK(lease_until BETWEEN 0 AND 9007199254740991),
 phase text NOT NULL DEFAULT 'search' CHECK(phase IN ('search','candidates','poll','needs_review')),
 page_id text REFERENCES payment_search_pages(id),
 candidate_index integer NOT NULL DEFAULT 0 CHECK(candidate_index BETWEEN 0 AND 10),
 operation bigint NOT NULL DEFAULT 0 CHECK(operation BETWEEN 0 AND 1500),
 polls integer NOT NULL DEFAULT 0 CHECK(polls BETWEEN 0 AND 10),
 failures integer NOT NULL DEFAULT 0 CHECK(failures BETWEEN 0 AND 5),
 review_reason text CHECK(review_reason IN ('unresolved','ambiguous','paid_observed','scan_limit','poll_limit','step_limit','custody_invalid','retry_exhausted')),
 CHECK((phase='needs_review')=(review_reason IS NOT NULL)),
 CHECK(phase NOT IN ('candidates','poll') OR page_id IS NOT NULL),
 CHECK(operation<=generation),
 CHECK(failures<5 OR phase='needs_review'),
 CHECK(polls<10 OR phase='needs_review'),
 CHECK(lease_until IS NULL OR (generation>0 AND lease_until>updated_at AND lease_until<=updated_at+60)),
 CHECK(phase<>'needs_review' OR lease_until IS NULL)
);
CREATE INDEX payment_reconciliation_due ON payment_reconciliation_jobs(due_at,attempt_id) WHERE phase<>'needs_review';
CREATE FUNCTION guard_payment_reconciliation_job() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN RAISE EXCEPTION 'Recovery jobs retain their custody' USING ERRCODE='23514'; END IF;
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_dispatch_starts s ON s.attempt_id=NEW.attempt_id JOIN payment_attempts a ON a.id=s.attempt_id
   WHERE e.id=NEW.source_event_id AND e.kind='payment.reconciliation_required.v1' AND e.aggregate_id=a.order_id
    AND e.created_at=s.started_at AND NEW.created_at=s.started_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id)) THEN
  RAISE EXCEPTION 'Recovery requires original atomic dispatch event' USING ERRCODE='23514';
 END IF;
 IF TG_OP='INSERT' AND (NEW.phase<>'search' OR NEW.generation<>0 OR NEW.lease_until IS NOT NULL OR NEW.page_id IS NOT NULL
   OR NEW.candidate_index<>0 OR NEW.operation<>0 OR NEW.polls<>0 OR NEW.failures<>0 OR NEW.review_reason IS NOT NULL OR NEW.due_at<>NEW.updated_at) THEN
  RAISE EXCEPTION 'Recovery starts as explicit pending discovery' USING ERRCODE='23514';
 END IF;
 IF NEW.page_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM payment_search_pages p WHERE p.id=NEW.page_id AND p.attempt_id=NEW.attempt_id) THEN
  RAISE EXCEPTION 'Recovery page belongs to another attempt' USING ERRCODE='23514';
 END IF;
 IF TG_OP='UPDATE' AND (NEW.attempt_id<>OLD.attempt_id OR NEW.source_event_id<>OLD.source_event_id OR NEW.created_at<>OLD.created_at
   OR NEW.updated_at<OLD.updated_at OR NEW.generation<OLD.generation OR NEW.generation>OLD.generation+1
   OR NEW.operation<OLD.operation OR NEW.polls<OLD.polls OR OLD.phase='needs_review') THEN
  RAISE EXCEPTION 'Recovery history cannot be rebound, rewound or silently restarted' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_reconciliation_job BEFORE INSERT OR UPDATE OR DELETE ON payment_reconciliation_jobs FOR EACH ROW EXECUTE FUNCTION guard_payment_reconciliation_job();
