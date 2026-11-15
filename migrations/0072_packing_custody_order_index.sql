-- Bound reciprocal packing-event inspection by order, including empty histories.
CREATE INDEX commerce_outbox_packing_custody_order
 ON commerce_outbox(aggregate_id)
 WHERE kind='order.packing_custody_recorded.v1';
