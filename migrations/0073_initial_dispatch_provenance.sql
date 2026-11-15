-- Private source custody only. Existing starts and observations remain unproven.
-- Transaction IDs are assigned to new rows; no historical origin is inferred.
ALTER TABLE public.payment_dispatch_starts ADD COLUMN source_transaction_id xid8;
ALTER TABLE public.payment_observations ADD COLUMN source_transaction_id xid8;

CREATE FUNCTION public.assign_payment_source_transaction() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF NEW.source_transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Payment source transaction is database-assigned' USING ERRCODE='23514';
 END IF;
 NEW.source_transaction_id=pg_catalog.pg_current_xact_id();
 RETURN NEW;
END $$;
CREATE TRIGGER aa_payment_start_source_transaction BEFORE INSERT ON public.payment_dispatch_starts
 FOR EACH ROW EXECUTE FUNCTION public.assign_payment_source_transaction();
CREATE TRIGGER aa_payment_observation_source_transaction BEFORE INSERT ON public.payment_observations
 FOR EACH ROW EXECUTE FUNCTION public.assign_payment_source_transaction();

-- IDs only: no copied provider ID, response body or checkout link.
CREATE TABLE public.payment_initial_dispatch_provenance (
 attempt_id text PRIMARY KEY REFERENCES public.payment_dispatch_starts(attempt_id),
 start_id text NOT NULL UNIQUE REFERENCES public.payment_dispatch_starts(id),
 observation_id text NOT NULL UNIQUE REFERENCES public.payment_observations(id),
 observation_event_id text NOT NULL UNIQUE REFERENCES public.commerce_outbox(id),
 observation_sha256 bytea NOT NULL CHECK(pg_catalog.octet_length(observation_sha256)=32),
 transaction_id xid8 NOT NULL
);
-- Hash a closed, ordered JSONB array, with an explicit domain and every stored
-- observation field. Bound the snapshot before building its canonical bytes.
CREATE FUNCTION public.payment_initial_observation_digest(o public.payment_observations) RETURNS bytea
 LANGUAGE sql IMMUTABLE SET search_path = pg_catalog, public, pg_temp AS $$
 SELECT CASE WHEN pg_catalog.octet_length(o.id)<=32 AND pg_catalog.octet_length(o.attempt_id)<=32
  AND pg_catalog.octet_length(o.effect_key)<=128 AND pg_catalog.octet_length(o.provider_id)<=256
  AND pg_catalog.octet_length(o.credential_fingerprint)<=64 AND pg_catalog.octet_length(o.status)<=16
  AND pg_catalog.octet_length(o.snapshot::text)<=8192 THEN
  pg_catalog.sha256(pg_catalog.convert_to(pg_catalog.jsonb_build_array('piwas:payment-initial-observation:v1',o.id,o.attempt_id,
   o.effect_key,o.revision,o.recorded_at,o.provider_id,o.credential_fingerprint,o.status,o.snapshot,
   o.source_transaction_id::text)::text,'UTF8'))
  ELSE NULL END;
$$;
CREATE FUNCTION public.payment_initial_dispatch_provenance_exact(p public.payment_initial_dispatch_provenance)
 RETURNS boolean LANGUAGE sql STABLE SET search_path = pg_catalog, public, pg_temp AS $$
 SELECT EXISTS(
  SELECT 1 FROM public.payment_dispatch_starts s
  JOIN public.payment_attempts a ON a.id=s.attempt_id
  JOIN public.payment_observations o ON o.id=p.observation_id AND o.attempt_id=s.attempt_id
  JOIN public.commerce_outbox e ON e.id=p.observation_event_id
  WHERE s.attempt_id=p.attempt_id AND s.id=p.start_id
   AND s.source_transaction_id IS NOT NULL AND s.source_transaction_id<>p.transaction_id
   AND o.source_transaction_id=p.transaction_id AND o.recorded_at>=s.started_at
   AND o.effect_key='initial-fetch-'||s.attempt_id
   AND p.observation_sha256=public.payment_initial_observation_digest(o)
   AND e.aggregate_id=a.order_id AND e.kind='payment.observation_recorded.v1'
   AND e.created_at=o.recorded_at
   AND e.payload=pg_catalog.jsonb_build_object('schema_version',1,'order_id',a.order_id,
      'attempt_id',a.id,'observation_id',o.id)
   AND EXISTS(SELECT 1 FROM public.commerce_outbox recovery
    WHERE recovery.aggregate_id=a.order_id AND recovery.kind='payment.reconciliation_required.v1'
     AND recovery.created_at=s.started_at
     AND recovery.payload=pg_catalog.jsonb_build_object('schema_version',1,'order_id',a.order_id,
       'attempt_id',a.id,'dispatch_id',s.id))
 );
$$;
CREATE FUNCTION public.guard_payment_initial_dispatch_provenance() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN
  RAISE EXCEPTION 'Initial dispatch provenance is immutable' USING ERRCODE='23514';
 END IF;
 IF pg_catalog.current_setting('transaction_isolation')<>'read committed' OR NEW.transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Initial provenance requires a database-assigned transaction' USING ERRCODE='23514';
 END IF;
 PERFORM 1 FROM public.payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 NEW.transaction_id=pg_catalog.pg_current_xact_id();
 IF NOT public.payment_initial_dispatch_provenance_exact(NEW) THEN
  RAISE EXCEPTION 'Initial provenance requires its exact new observation and prior start' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER guard_payment_initial_dispatch_provenance BEFORE INSERT OR UPDATE OR DELETE
 ON public.payment_initial_dispatch_provenance FOR EACH ROW EXECUTE FUNCTION public.guard_payment_initial_dispatch_provenance();
CREATE FUNCTION public.require_payment_initial_dispatch_provenance() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_catalog.pg_current_xact_id()
  OR NOT public.payment_initial_dispatch_provenance_exact(NEW) THEN
  RAISE EXCEPTION 'Initial provenance requires same-transaction custody' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_payment_initial_dispatch_provenance
 AFTER INSERT ON public.payment_initial_dispatch_provenance DEFERRABLE INITIALLY DEFERRED
 FOR EACH ROW EXECUTE FUNCTION public.require_payment_initial_dispatch_provenance();
