-- Private account-aware test payment custody. Authentication remains in Rust;
-- the native boundary binds the recorded account to the exact accepted quote.
ALTER TABLE payment_attempts
 ADD COLUMN promotion_account_id bigint REFERENCES verified_customer_accounts(id);

-- Preserve the guest-link restriction: only payment admission is account-aware.
DROP TRIGGER aa_account_order_guest_only ON payment_attempts;
CREATE FUNCTION guard_payment_promotion_account() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(
  SELECT 1 FROM checkout_orders o
  JOIN guest_quote_snapshots q ON q.id=o.quote_id AND q.basket_id=o.basket_id
  WHERE o.id=NEW.order_id AND o.basket_id=NEW.basket_id
   AND q.promotion_account_id IS NOT DISTINCT FROM NEW.promotion_account_id
 ) THEN
  RAISE EXCEPTION 'Payment requires exact order and quote account custody' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_payment_promotion_account BEFORE INSERT ON payment_attempts
 FOR EACH ROW EXECUTE FUNCTION guard_payment_promotion_account();
