-- Discovery custody only. No end page proves absence or authorizes another charge.
CREATE TABLE payment_search_pages (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 revision bigint NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 page_number bigint NOT NULL CHECK(page_number BETWEEN 1 AND 100),
 recorded_at bigint NOT NULL CHECK(recorded_at BETWEEN 0 AND 9007199254740991),
 previous_page_id text UNIQUE REFERENCES payment_search_pages(id),
 from_cursor text CHECK(from_cursor ~ '^tr_.+$' AND octet_length(from_cursor)<=256 AND btrim(from_cursor)=from_cursor AND from_cursor !~ '[[:cntrl:]]'),
 next_cursor text CHECK(next_cursor ~ '^tr_.+$' AND octet_length(next_cursor)<=256 AND btrim(next_cursor)=next_cursor AND next_cursor !~ '[[:cntrl:]]'),
 candidates jsonb NOT NULL CHECK(jsonb_typeof(candidates)='array' AND octet_length(candidates::text)<=8192),
 credential_fingerprint text NOT NULL CHECK(credential_fingerprint ~ '^[a-f0-9]{64}$'),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=8192),
 UNIQUE(attempt_id,effect_key), UNIQUE(attempt_id,revision),
 CHECK((previous_page_id IS NULL)=(from_cursor IS NULL)),
 CHECK((page_number=1)=(previous_page_id IS NULL)),
 CHECK(from_cursor IS NULL OR next_cursor IS NULL OR from_cursor<>next_cursor)
);
CREATE FUNCTION guard_payment_search_page() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE a payment_attempts%ROWTYPE; started bigint; last_revision bigint; last_at bigint;
 p payment_search_pages%ROWTYPE; parent_id text; expected jsonb; cursor_count bigint; traversed integer=0; child_number bigint;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Search pages are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO a FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT started_at INTO started FROM payment_dispatch_starts WHERE attempt_id=NEW.attempt_id;
 SELECT revision,recorded_at INTO last_revision,last_at FROM payment_search_pages WHERE attempt_id=NEW.attempt_id ORDER BY revision DESC LIMIT 1;
 IF a.id IS NULL OR started IS NULL OR NEW.recorded_at<started OR NEW.recorded_at<coalesce(last_at,0) OR NEW.revision<>coalesce(last_revision,0)+1 THEN
  RAISE EXCEPTION 'Search requires original dispatch and contiguous history' USING ERRCODE='23514';
 END IF;
 IF jsonb_array_length(NEW.candidates)>10 OR EXISTS(SELECT 1 FROM jsonb_array_elements(NEW.candidates) v WHERE jsonb_typeof(v)<>'string' OR v#>>'{}' !~ '^tr_.+$' OR octet_length(v#>>'{}')>256 OR btrim(v#>>'{}')<>v#>>'{}' OR v#>>'{}' ~ '[[:cntrl:]]') THEN
  RAISE EXCEPTION 'Invalid discovery candidates' USING ERRCODE='23514';
 END IF;
 SELECT count(DISTINCT v) INTO cursor_count FROM jsonb_array_elements(NEW.candidates) v;
 IF cursor_count<>jsonb_array_length(NEW.candidates) THEN RAISE EXCEPTION 'Duplicate discovery candidates' USING ERRCODE='23514'; END IF;
 parent_id=NEW.previous_page_id;
 IF parent_id IS NOT NULL THEN
  SELECT * INTO p FROM payment_search_pages WHERE id=parent_id;
  IF p.id IS NULL OR p.attempt_id<>NEW.attempt_id OR p.next_cursor IS NULL OR NEW.from_cursor IS DISTINCT FROM p.next_cursor OR NEW.page_number<>p.page_number+1 OR NEW.revision<=p.revision OR NEW.recorded_at<p.recorded_at THEN
   RAISE EXCEPTION 'Search must continue exact predecessor' USING ERRCODE='23514';
  END IF;
  LOOP
   traversed=traversed+1;
   IF traversed>100 THEN RAISE EXCEPTION 'Search ancestry exceeds bound' USING ERRCODE='23514'; END IF;
   IF NEW.next_cursor IS NOT NULL AND NEW.next_cursor=p.from_cursor THEN RAISE EXCEPTION 'Cyclic search cursor' USING ERRCODE='23514'; END IF;
   EXIT WHEN p.previous_page_id IS NULL;
   child_number=p.page_number;
   parent_id=p.previous_page_id;
   SELECT * INTO p FROM payment_search_pages WHERE id=parent_id;
   IF p.id IS NULL OR p.page_number<>child_number-1 OR p.attempt_id<>NEW.attempt_id THEN RAISE EXCEPTION 'Invalid search ancestor' USING ERRCODE='23514'; END IF;
  END LOOP;
 END IF;
 expected=jsonb_build_object('schema_version',1,'id',NEW.id,'attempt_id',a.id,'effect_key',NEW.effect_key,'revision',NEW.revision,'page_number',NEW.page_number,'recorded_at',NEW.recorded_at,'intent',jsonb_build_object('previous_page_id',NEW.previous_page_id,'from',NEW.from_cursor,'next',NEW.next_cursor,'candidates',NEW.candidates,'credential_fingerprint',NEW.credential_fingerprint));
 IF NEW.snapshot IS DISTINCT FROM expected THEN RAISE EXCEPTION 'Search requires exact minimal snapshot' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_payment_search_page BEFORE INSERT OR UPDATE OR DELETE ON payment_search_pages FOR EACH ROW EXECUTE FUNCTION guard_payment_search_page();
DROP INDEX commerce_outbox_original_order_kind;
CREATE UNIQUE INDEX commerce_outbox_original_order_kind ON commerce_outbox(aggregate_id,kind) WHERE kind NOT IN ('payment.observation_recorded.v1','payment.search_page_recorded.v1');
CREATE UNIQUE INDEX commerce_outbox_search_page_identity ON commerce_outbox((payload->>'page_id')) WHERE kind='payment.search_page_recorded.v1';
ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1'));
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
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE FUNCTION require_complete_payment_search_page() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox e JOIN payment_attempts a ON a.order_id=e.aggregate_id WHERE a.id=NEW.attempt_id AND e.kind='payment.search_page_recorded.v1' AND e.created_at=NEW.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'page_id',NEW.id)) THEN
  RAISE EXCEPTION 'Search page requires durable processing custody' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_payment_search_page AFTER INSERT ON payment_search_pages DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_payment_search_page();
