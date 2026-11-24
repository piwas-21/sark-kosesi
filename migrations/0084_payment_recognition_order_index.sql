-- Scope reciprocal gross-recognition events by order, including an empty journal.
CREATE INDEX commerce_outbox_payment_recognition_order
 ON commerce_outbox(aggregate_id)
 WHERE kind='payment.recognized.v1';
