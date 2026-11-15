-- A local start is not a delivery receipt or authority to retry a message.
CREATE TABLE guest_link_delivery_starts (
 verifier bytea PRIMARY KEY REFERENCES guest_link_delivery_envelopes(verifier),
 key_id text NOT NULL REFERENCES guest_link_delivery_keys(key_id),
 nonce_counter bigint NOT NULL,
 key_fingerprint bytea NOT NULL CHECK(octet_length(key_fingerprint)=32),
 started_at bigint NOT NULL CHECK(started_at BETWEEN 0 AND 9007199254740991),
 transaction_id xid8 NOT NULL,
 FOREIGN KEY(key_id,nonce_counter) REFERENCES guest_link_delivery_nonces(key_id,nonce_counter)
);
CREATE FUNCTION guest_link_delivery_start_source(s guest_link_delivery_starts) RETURNS boolean
 LANGUAGE sql VOLATILE AS $$
 SELECT EXISTS(
  SELECT 1 FROM guest_order_link_proofs p
  JOIN checkout_orders o ON o.id=p.order_id AND o.basket_id=p.basket_id
  JOIN guest_quote_snapshots q ON q.id=o.quote_id AND q.basket_id=o.basket_id AND q.promotion_account_id IS NULL
  JOIN guest_link_delivery_envelopes e ON e.verifier=p.verifier
  JOIN guest_link_delivery_nonces n ON n.key_id=e.key_id AND n.nonce_counter=e.nonce_counter AND n.verifier=p.verifier
  JOIN guest_link_delivery_keys k ON k.key_id=n.key_id
  WHERE p.verifier=s.verifier AND p.delivery_mode='envelope'
   AND p.consumed_at IS NULL AND p.consumed_account_id IS NULL AND p.consumed_xid IS NULL
   AND NOT EXISTS(SELECT 1 FROM customer_order_links l WHERE l.order_id=p.order_id)
   AND p.created_at<=s.started_at AND s.started_at<p.expires_at
   AND n.reserved_at BETWEEN k.registered_at AND p.created_at
   AND e.key_id=s.key_id AND e.nonce_counter=s.nonce_counter AND k.key_fingerprint=s.key_fingerprint
   AND e.envelope_version=1 AND octet_length(e.ciphertext)=48
   AND p.delivery_transaction_id=e.transaction_id AND e.transaction_id<>n.transaction_id
   AND e.transaction_id<>s.transaction_id AND n.transaction_id<>s.transaction_id
 );
$$;
CREATE FUNCTION guard_guest_link_delivery_start() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE source_order text; source_basket text;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Guest link starts are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' OR current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' THEN
  RAISE EXCEPTION 'Guest link start requires durable read committed' USING ERRCODE='23514'; END IF;
 IF NEW.transaction_id IS NOT NULL THEN RAISE EXCEPTION 'Start transaction is assigned by database' USING ERRCODE='23514'; END IF;
 SELECT order_id,basket_id INTO source_order,source_basket FROM guest_order_link_proofs WHERE verifier=NEW.verifier FOR UPDATE;
 PERFORM 1 FROM checkout_orders WHERE id=source_order AND basket_id=source_basket FOR UPDATE;
 NEW.transaction_id=pg_current_xact_id();
 IF NOT guest_link_delivery_start_source(NEW) THEN
  RAISE EXCEPTION 'Start requires its exact live guest link source' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER guard_guest_link_delivery_start BEFORE INSERT OR UPDATE OR DELETE ON guest_link_delivery_starts
 FOR EACH ROW EXECUTE FUNCTION guard_guest_link_delivery_start();
CREATE FUNCTION require_guest_link_delivery_start() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF current_setting('fsync')<>'on' OR current_setting('synchronous_commit')<>'on' OR NEW.transaction_id IS DISTINCT FROM pg_current_xact_id() OR NOT guest_link_delivery_start_source(NEW) THEN
  RAISE EXCEPTION 'Guest link start requires its exact durable source' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER require_guest_link_delivery_start AFTER INSERT ON guest_link_delivery_starts
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_guest_link_delivery_start();
CREATE FUNCTION seal_guest_link_delivery_start_source() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM guest_link_delivery_starts WHERE verifier=OLD.verifier AND transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Start and consumption require separate transactions' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER seal_guest_link_delivery_start_source BEFORE UPDATE ON guest_order_link_proofs
 FOR EACH ROW EXECUTE FUNCTION seal_guest_link_delivery_start_source();
-- A different proof for the same order cannot bypass an early constraint flush.
CREATE FUNCTION seal_guest_link_delivery_start_order() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM guest_link_delivery_starts s JOIN guest_order_link_proofs p ON p.verifier=s.verifier
  WHERE p.order_id=NEW.order_id AND s.transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Start and order linkage require separate transactions' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER seal_guest_link_delivery_start_order BEFORE INSERT ON customer_order_links
 FOR EACH ROW EXECUTE FUNCTION seal_guest_link_delivery_start_order();
