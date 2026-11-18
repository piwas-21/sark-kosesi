-- Stable bytewise traversal of private pending-request identities.
CREATE INDEX checkout_post_dispatch_request_order_c
 ON checkout_post_dispatch_requests (order_id COLLATE "C");
