SELECT count(*) FROM (
 SELECT 1 FROM commerce_outbox
 WHERE aggregate_id=$1 AND kind='payment.paid_parent_totals_observed.v1'
 LIMIT 129
) AS scoped
