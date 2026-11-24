-- Scope reciprocal paid-parent receipt events even for an empty order journal.
CREATE INDEX commerce_outbox_paid_parent_totals_order
 ON commerce_outbox(aggregate_id)
 WHERE kind='payment.paid_parent_totals_observed.v1';
