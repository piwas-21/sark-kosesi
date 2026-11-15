-- A marked lookup start and original-attempt supervisor job cannot coexist.
-- Both insert paths serialize on the referenced dispatch row, before locking
-- the lookup operation. No database lock survives the caller's transaction.
CREATE FUNCTION public.payment_test_continuation_fenced(attempt text) RETURNS boolean LANGUAGE sql STABLE SET search_path = pg_catalog, public, pg_temp AS $$
 SELECT EXISTS(SELECT 1 FROM public.payment_test_continuations c
  JOIN public.callback_lookup_operations o ON o.id=c.operation_id
  JOIN public.callback_lookup_fetch_starts s ON s.operation_id=o.id
  WHERE c.attempt_id=attempt AND s.started_at>=o.created_at
  AND public.payment_test_continuation_exact(c));
$$;
CREATE FUNCTION public.guard_test_continuation_fetch_fence() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
DECLARE attempt text; mapped_attempt text;
BEGIN
 SELECT o.continuation_attempt_id,c.attempt_id INTO attempt,mapped_attempt
 FROM public.callback_lookup_operations o LEFT JOIN public.payment_test_continuations c ON c.operation_id=o.id
 WHERE o.id=NEW.operation_id;
 IF attempt IS NULL AND mapped_attempt IS NULL THEN RETURN NEW; END IF;
 IF attempt IS DISTINCT FROM mapped_attempt THEN
  RAISE EXCEPTION 'Continuation arbitration unavailable' USING ERRCODE='23514'; END IF;
 IF pg_catalog.current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Continuation arbitration requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM public.payment_dispatch_starts WHERE attempt_id=attempt FOR UPDATE;
 IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM public.payment_test_continuations c
  WHERE c.attempt_id=attempt AND c.operation_id=NEW.operation_id AND public.payment_test_continuation_exact(c))
  OR EXISTS(SELECT 1 FROM public.payment_reconciliation_jobs j WHERE j.attempt_id=attempt) THEN
  RAISE EXCEPTION 'Continuation arbitration unavailable' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER aa_test_continuation_fetch_fence BEFORE INSERT ON public.callback_lookup_fetch_starts
 FOR EACH ROW EXECUTE FUNCTION public.guard_test_continuation_fetch_fence();

CREATE FUNCTION public.guard_test_continuation_supervisor_fence() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
BEGIN
 IF pg_catalog.current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Supervisor arbitration requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM public.payment_dispatch_starts WHERE attempt_id=NEW.attempt_id FOR UPDATE;
 IF NOT FOUND THEN
  RAISE EXCEPTION 'Supervisor arbitration unavailable' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM public.callback_lookup_operations o
  JOIN public.callback_lookup_fetch_starts s ON s.operation_id=o.id
  WHERE o.continuation_attempt_id=NEW.attempt_id)
 OR EXISTS(SELECT 1 FROM public.payment_test_continuations c
  JOIN public.callback_lookup_fetch_starts s ON s.operation_id=c.operation_id
  WHERE c.attempt_id=NEW.attempt_id) THEN
  IF NOT public.payment_test_continuation_fenced(NEW.attempt_id) THEN
   RAISE EXCEPTION 'Invalid continuation arbitration custody' USING ERRCODE='23514'; END IF;
  RETURN NULL;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER aa_test_continuation_supervisor_fence BEFORE INSERT ON public.payment_reconciliation_jobs
 FOR EACH ROW EXECUTE FUNCTION public.guard_test_continuation_supervisor_fence();
