-- Keep the private history seek scoped to one account and a stable total order.
CREATE INDEX customer_order_links_account_history
    ON customer_order_links(account_id, linked_at DESC, order_id COLLATE "C" DESC);
