-- The attempt is already claimed and its current recognition prefix proved.
-- Limit before any source proof; reuse each historical recognition once.
WITH bounded AS MATERIALIZED (
 SELECT o.* FROM paid_parent_total_operations o WHERE o.attempt_id=$1
 ORDER BY o.id COLLATE "C" LIMIT 129
), source_ids AS MATERIALIZED (
 SELECT DISTINCT recognition_id FROM bounded
), sources AS MATERIALIZED (
 SELECT r.id,r.order_id,r.attempt_id,r.through_observation,r.recorded_at,r.state,
   i.summary,
   coalesce(payment_recognition_source_after_prefix(r,false,$2),false)
   AND NOT EXISTS(SELECT 1 FROM payment_recognitions previous
     WHERE previous.order_id=r.order_id AND previous.through_observation<r.through_observation
       AND previous.recorded_at>r.recorded_at)
   AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=r.order_id
     AND e.kind='payment.recognized.v1' AND e.created_at=r.recorded_at
     AND e.payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,
       'attempt_id',r.attempt_id,'recognition_id',r.id,
       'recognition_sha256',payment_recognition_digest(r))) AS valid
 FROM source_ids s JOIN payment_recognitions r ON r.id=s.recognition_id
 LEFT JOIN payment_identity_resolutions i ON i.id=r.resolution_id
)
SELECT coalesce(paid_parent_total_operation_shape(o) AND s.valid
 AND s.order_id=o.order_id AND s.attempt_id=o.attempt_id AND s.state='received'
 AND s.through_observation=o.through_observation AND o.started_at>=s.recorded_at
 AND o.profile_id=a.configuration->'profile'->>'id'
 AND jsonb_typeof(s.summary->'provider_ids')='array'
 AND jsonb_typeof(s.summary->'paid_provider_ids')='array'
 AND s.summary->'provider_ids'=jsonb_build_array(o.payment_id)
 AND s.summary->'paid_provider_ids'=jsonb_build_array(o.payment_id)
 AND (r.operation_id IS NULL OR (paid_parent_total_receipt_shape(r,o)
  AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=r.event_id
   AND e.aggregate_id=o.order_id AND e.kind='payment.paid_parent_totals_observed.v1'
   AND e.created_at=r.recorded_at AND e.payload=paid_parent_total_receipt_event(r,o)))),false) AS valid,
 (r.operation_id IS NOT NULL) AS complete,
 encode(sha256(convert_to(jsonb_build_array(o.id,o.order_id,o.attempt_id,o.effect_key,
   o.recognition_id,o.through_observation,o.profile_id,o.payment_id,o.credential_fingerprint,
   o.started_at,o.fetch_before,o.transaction_id::text,
   CASE WHEN r.operation_id IS NULL THEN NULL ELSE jsonb_build_array(r.operation_id,
    r.refunded_minor,r.remaining_minor,r.captured_minor,r.charged_back_minor,
    r.fetched_at,r.recorded_at,r.event_id,r.transaction_id::text) END)::text,'UTF8')),'hex') AS digest
FROM bounded o
LEFT JOIN sources s ON s.id=o.recognition_id
LEFT JOIN payment_attempts a ON a.id=o.attempt_id
LEFT JOIN paid_parent_total_receipts r ON r.operation_id=o.id
ORDER BY o.id COLLATE "C"
