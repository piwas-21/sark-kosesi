-- Historical source only: current redemption or expiry must not erase an outcome.
SELECT s.verifier IS NOT NULL, o.verifier IS NOT NULL, left(o.outcome,20),
 COALESCE(o.start_transaction_id=s.transaction_id AND o.transaction_id<>s.transaction_id
  AND o.observed_at>=s.started_at AND o.observed_at<=9007199254740991,false),
 COALESCE(EXISTS(
  SELECT 1 FROM guest_order_link_proofs p
  JOIN checkout_orders ord ON ord.id=p.order_id AND ord.basket_id=p.basket_id
  JOIN guest_quote_snapshots q ON q.id=ord.quote_id AND q.basket_id=ord.basket_id AND q.promotion_account_id IS NULL
  JOIN guest_link_delivery_envelopes e ON e.verifier=p.verifier
  JOIN guest_link_delivery_nonces n ON n.key_id=e.key_id AND n.nonce_counter=e.nonce_counter AND n.verifier=p.verifier
  JOIN guest_link_delivery_keys k ON k.key_id=n.key_id
  WHERE p.verifier=s.verifier AND p.delivery_mode='envelope'
   AND p.created_at>=0 AND p.expires_at-p.created_at=900
   AND p.created_at<=s.started_at AND s.started_at<p.expires_at
   AND n.reserved_at BETWEEN k.registered_at AND p.created_at
   AND e.key_id=s.key_id AND e.nonce_counter=s.nonce_counter AND k.key_fingerprint=s.key_fingerprint
   AND e.envelope_version=1 AND octet_length(e.ciphertext)=48
   AND p.delivery_transaction_id=e.transaction_id AND e.transaction_id<>n.transaction_id
   AND e.transaction_id<>s.transaction_id AND n.transaction_id<>s.transaction_id
 ),false)
FROM (SELECT $1::bytea AS verifier) requested
LEFT JOIN guest_link_delivery_starts s ON s.verifier=requested.verifier
LEFT JOIN guest_link_delivery_outcomes o ON o.verifier=requested.verifier
