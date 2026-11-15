-- A private local operation is not a provider request or financial authority.
-- Existing generic callback operations remain valid and have no inferred origin.
ALTER TABLE public.callback_lookup_operations ADD COLUMN source_transaction_id xid8;
ALTER TABLE public.callback_lookup_operations ADD COLUMN continuation_attempt_id text REFERENCES public.payment_initial_dispatch_provenance(attempt_id);
CREATE UNIQUE INDEX callback_lookup_continuation_attempt ON public.callback_lookup_operations(continuation_attempt_id)
 WHERE continuation_attempt_id IS NOT NULL;
CREATE FUNCTION public.assign_callback_operation_source_transaction() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF NEW.source_transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Callback operation transaction is database-assigned' USING ERRCODE='23514'; END IF;
 NEW.source_transaction_id=pg_catalog.pg_current_xact_id();
 RETURN NEW;
END $$;
CREATE TRIGGER aa_callback_operation_source_transaction BEFORE INSERT ON public.callback_lookup_operations
 FOR EACH ROW EXECUTE FUNCTION public.assign_callback_operation_source_transaction();

-- Exactly one fresh lookup operation can be associated with the original source.
-- The association stores IDs and a transaction identity, never provider data.
CREATE TABLE public.payment_test_continuations (
 attempt_id text PRIMARY KEY REFERENCES public.payment_initial_dispatch_provenance(attempt_id),
 operation_id text NOT NULL UNIQUE REFERENCES public.callback_lookup_operations(id),
 transaction_id xid8 NOT NULL
);
CREATE FUNCTION public.payment_test_continuation_exact(c public.payment_test_continuations)
 RETURNS boolean LANGUAGE sql STABLE SET search_path = pg_catalog, public, pg_temp AS $$
 SELECT EXISTS(
  SELECT 1 FROM public.payment_initial_dispatch_provenance p
  JOIN public.payment_attempts a ON a.id=p.attempt_id
  JOIN public.payment_observations o ON o.id=p.observation_id AND o.attempt_id=p.attempt_id
  JOIN public.callback_lookup_operations op ON op.id=c.operation_id
  WHERE p.attempt_id=c.attempt_id AND public.payment_initial_dispatch_provenance_exact(p)
   AND op.source_transaction_id=c.transaction_id AND op.continuation_attempt_id=c.attempt_id
   AND p.transaction_id<>c.transaction_id
   AND op.profile_id=a.snapshot->'binding'->'profile'->>'id'
   AND op.provider_id=o.provider_id AND op.created_at>=o.recorded_at
 );
$$;
CREATE FUNCTION public.guard_payment_test_continuation() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN
  RAISE EXCEPTION 'Test continuation custody is immutable' USING ERRCODE='23514'; END IF;
 IF pg_catalog.current_setting('transaction_isolation')<>'read committed' OR NEW.transaction_id IS NOT NULL THEN
  RAISE EXCEPTION 'Test continuation requires a database-assigned transaction' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM public.payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 NEW.transaction_id=pg_catalog.pg_current_xact_id();
 IF NOT public.payment_test_continuation_exact(NEW) THEN
  RAISE EXCEPTION 'Test continuation requires its exact fresh operation and source' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER guard_payment_test_continuation BEFORE INSERT OR UPDATE OR DELETE
 ON public.payment_test_continuations FOR EACH ROW EXECUTE FUNCTION public.guard_payment_test_continuation();
CREATE FUNCTION public.require_payment_test_continuation() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF NEW.transaction_id IS DISTINCT FROM pg_catalog.pg_current_xact_id()
  OR NOT public.payment_test_continuation_exact(NEW) THEN
  RAISE EXCEPTION 'Test continuation requires same-transaction custody' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_payment_test_continuation AFTER INSERT ON public.payment_test_continuations
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.require_payment_test_continuation();
CREATE FUNCTION public.require_callback_test_continuation() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM public.payment_test_continuations c WHERE c.operation_id=NEW.id
  AND c.attempt_id=NEW.continuation_attempt_id AND c.transaction_id=pg_catalog.pg_current_xact_id()
  AND public.payment_test_continuation_exact(c)) THEN
  RAISE EXCEPTION 'Marked callback operation requires its same-transaction source' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_callback_test_continuation AFTER INSERT ON public.callback_lookup_operations
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.continuation_attempt_id IS NOT NULL)
 EXECUTE FUNCTION public.require_callback_test_continuation();
