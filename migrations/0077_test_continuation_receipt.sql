-- Structural source binding only: provider authentication remains a sealed
-- Rust capability. Generic callback receipts retain their existing semantics.
CREATE FUNCTION public.guard_test_continuation_receipt() RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, public, pg_temp AS $$
DECLARE marked text; mapped text;
BEGIN
 SELECT o.continuation_attempt_id,c.attempt_id INTO marked,mapped
 FROM public.callback_lookup_operations o LEFT JOIN public.payment_test_continuations c ON c.operation_id=o.id
 WHERE o.id=NEW.operation_id;
 IF marked IS NULL AND mapped IS NULL THEN RETURN NEW; END IF;
 IF marked IS DISTINCT FROM mapped OR pg_catalog.current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Continuation receipt custody invalid' USING ERRCODE='23514'; END IF;
 -- This precedes the existing receipt guard's operation lock.
 PERFORM 1 FROM public.payment_attempts WHERE id=marked FOR UPDATE;
 PERFORM 1 FROM public.payment_dispatch_starts WHERE attempt_id=marked FOR UPDATE;
 PERFORM 1 FROM public.callback_lookup_operations WHERE id=NEW.operation_id FOR UPDATE;
 IF NOT EXISTS(SELECT 1 FROM public.payment_test_continuations c
  JOIN public.callback_lookup_operations o ON o.id=c.operation_id
  JOIN public.callback_lookup_fetch_starts s ON s.operation_id=o.id
  JOIN public.payment_attempts a ON a.id=c.attempt_id
  JOIN public.payment_initial_dispatch_provenance p ON p.attempt_id=c.attempt_id
  JOIN public.payment_observations initial_obs ON initial_obs.id=p.observation_id
  WHERE c.attempt_id=marked AND c.operation_id=NEW.operation_id
   AND public.payment_test_continuation_exact(c) AND NOT EXISTS(SELECT 1 FROM public.payment_reconciliation_jobs j WHERE j.attempt_id=marked)
   AND NEW.recorded_at>=s.started_at AND s.started_at>=o.created_at
   AND NEW.snapshot->'payment'->>'resource'='payment'
   AND NEW.snapshot->'payment'->>'id'=initial_obs.provider_id AND initial_obs.provider_id=o.provider_id
   AND NEW.snapshot->'payment'->>'profile_id'=o.profile_id
   AND NEW.snapshot->'payment'->>'mode'='test' AND NEW.snapshot->'payment'->>'currency'='EUR'
   AND NEW.snapshot->'payment'->'amount_minor'=a.snapshot->'binding'->'amount_minor'
   AND NEW.snapshot->'payment'->'metadata_schema_version'='1'::jsonb
   AND NEW.snapshot->'payment'->>'attempt_id'=a.id AND NEW.snapshot->'payment'->>'order_id'=a.order_id
   AND a.snapshot->'binding'->>'id'=a.id AND a.snapshot->'binding'->>'order_id'=a.order_id
   AND a.snapshot->'binding'->>'currency'='EUR') THEN
  RAISE EXCEPTION 'Continuation receipt custody invalid' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER aa_test_continuation_receipt BEFORE INSERT ON public.callback_lookup_receipts
 FOR EACH ROW EXECUTE FUNCTION public.guard_test_continuation_receipt();
