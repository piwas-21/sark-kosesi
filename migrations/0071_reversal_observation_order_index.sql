-- Bound reciprocal reversal event inspection by order even for empty journals.
CREATE INDEX commerce_outbox_reversal_observation_order
 ON commerce_outbox(aggregate_id)
 WHERE kind='payment.reversal_observed.v1';
