-- Private account-bound promotion usage shares the existing promotion anchor.
-- Account identity is authenticated by trusted Rust, not by this foreign key.
ALTER TABLE promotion_redemptions
 ADD COLUMN promotion_account_id bigint REFERENCES verified_customer_accounts(id),
 ADD COLUMN account_redemption_number bigint,
 ADD CONSTRAINT promotion_redemption_account_sequence CHECK (
  (promotion_account_id IS NULL AND account_redemption_number IS NULL) OR
  (promotion_account_id IS NOT NULL AND account_redemption_number IS NOT NULL
   AND account_redemption_number BETWEEN 1 AND 1000000));
CREATE UNIQUE INDEX promotion_redemptions_account_sequence
 ON promotion_redemptions(promotion_id,promotion_account_id,account_redemption_number)
 WHERE promotion_account_id IS NOT NULL;
CREATE INDEX promotion_redemptions_anonymous
 ON promotion_redemptions(promotion_id) WHERE promotion_account_id IS NULL;

-- The same-transaction debit and deferred checkout guard now bind this quote's
-- exact account, replacing the unconditional 0053 account-order blocker.
DROP TRIGGER aa_checkout_guest_quote_account ON checkout_orders;
DROP FUNCTION guard_checkout_guest_quote_account();

CREATE OR REPLACE FUNCTION guard_promotion_redemption() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE q guest_quote_snapshots%ROWTYPE; o checkout_orders%ROWTYPE;
 draft promotion_policy_drafts%ROWTYPE; review promotion_policy_reviews%ROWTYPE;
 limits jsonb; promotion jsonb; previous_count bigint; previous_discount bigint; account_count bigint;
 line_discount numeric; eligible_gross numeric; requested_discount numeric;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Promotion redemption is immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Promotion redemption requires read committed' USING ERRCODE='23514'; END IF;
 -- A direct UPDATE claim prevents lock upgrades between concurrent checkouts.
 PERFORM 1 FROM promotion_policy_anchors WHERE id=NEW.promotion_id FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Promotion anchor required' USING ERRCODE='23514'; END IF;
 -- Each SELECT follows the anchor wait in a fresh READ COMMITTED statement.
 SELECT * INTO q FROM guest_quote_snapshots WHERE id=NEW.quote_id;
 SELECT * INTO o FROM checkout_orders WHERE id=NEW.order_id;
 SELECT * INTO draft FROM promotion_policy_drafts WHERE id=NEW.promotion_id ORDER BY revision DESC LIMIT 1;
 SELECT * INTO review FROM promotion_policy_reviews WHERE id=NEW.promotion_id
  AND draft_revision=NEW.promotion_revision ORDER BY revision DESC LIMIT 1;
 IF q.id IS NULL OR o.id IS NULL OR draft.id IS NULL OR review.id IS NULL
  OR q.id IS DISTINCT FROM o.quote_id OR q.basket_id IS DISTINCT FROM o.basket_id
  OR q.basket_revision IS DISTINCT FROM o.basket_revision
  OR q.promotion_id IS DISTINCT FROM NEW.promotion_id
  OR q.promotion_revision IS DISTINCT FROM NEW.promotion_revision
  OR q.promotion_review_revision IS DISTINCT FROM NEW.promotion_review_revision
  OR q.promotion_account_id IS DISTINCT FROM NEW.promotion_account_id
  OR draft.revision IS DISTINCT FROM NEW.promotion_revision
  OR review.revision IS DISTINCT FROM NEW.promotion_review_revision
  OR review.decision IS DISTINCT FROM 'approved' OR review.actor IS NOT DISTINCT FROM draft.actor
  OR o.promotion_redemption_transaction_id IS DISTINCT FROM pg_current_xact_id()
  OR o.created_at IS DISTINCT FROM NEW.created_at
  OR o.snapshot->'quote' IS DISTINCT FROM q.snapshot
  OR q.snapshot->'schema_version' IS DISTINCT FROM '2'::jsonb
  OR q.snapshot->'promotion' IS DISTINCT FROM jsonb_build_object('id',NEW.promotion_id,'revision',NEW.promotion_revision,'review_revision',NEW.promotion_review_revision)
  OR q.snapshot->'discount_minor' IS DISTINCT FROM to_jsonb(NEW.discount_minor)
  OR NEW.created_at<q.created_at OR NEW.created_at>=q.expires_at THEN
  RAISE EXCEPTION 'Redemption requires the exact current approved order and quote' USING ERRCODE='23514'; END IF;
 IF NOT promoted_redemption_original_source(q) THEN
  RAISE EXCEPTION 'Redemption requires exact original quote source' USING ERRCODE='23514'; END IF;
 -- An imported quote must not turn an inconsistent or zero-payable gross
 -- allocation into a durable debit. Rust independently reconstructs the full
 -- original quote and its reviewed promotion before authorizing the order.
 IF NOT promoted_quote_amount_shape(q.snapshot->'total')
  OR NOT promoted_quote_amount_shape(q.snapshot->'original'->'total')
  OR jsonb_typeof(q.snapshot->'lines') IS DISTINCT FROM 'array'
  OR jsonb_typeof(q.snapshot->'original'->'lines') IS DISTINCT FROM 'array' THEN
  RAISE EXCEPTION 'Redemption requires bounded gross allocation' USING ERRCODE='23514'; END IF;
 IF jsonb_array_length(q.snapshot->'lines') NOT BETWEEN 1 AND 20
  OR jsonb_array_length(q.snapshot->'lines')<>jsonb_array_length(q.snapshot->'original'->'lines')
  OR (q.snapshot->'total'->>'gross_minor')::numeric<=0
  OR (q.snapshot->'original'->'total'->>'gross_minor')::numeric
    -(q.snapshot->'total'->>'gross_minor')::numeric<>NEW.discount_minor THEN
  RAISE EXCEPTION 'Redemption requires positive reconciled gross' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(q.snapshot->'lines') WITH ORDINALITY p(value,n)
  WHERE jsonb_typeof(p.value) IS DISTINCT FROM 'object'
   OR jsonb_typeof(p.value->'discount_minor') IS DISTINCT FROM 'number'
   OR coalesce(p.value->>'discount_minor','') !~ '^(0|[1-9][0-9]{0,15})$'
   OR NOT promoted_quote_amount_shape(p.value->'amounts')
   OR NOT promoted_quote_amount_shape(q.snapshot->'original'->'lines'->((p.n-1)::integer)->'amounts')
   OR p.value->'sku' IS DISTINCT FROM q.snapshot->'original'->'lines'->((p.n-1)::integer)->'sku') THEN
  RAISE EXCEPTION 'Redemption requires bounded line allocations' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(q.snapshot->'lines') WITH ORDINALITY p(value,n)
  WHERE (p.value->>'discount_minor')::numeric>9007199254740991
   OR (q.snapshot->'original'->'lines'->((p.n-1)::integer)->'amounts'->>'gross_minor')::numeric
    -(p.value->'amounts'->>'gross_minor')::numeric<>(p.value->>'discount_minor')::numeric) THEN
  RAISE EXCEPTION 'Redemption requires reconciled line allocations' USING ERRCODE='23514'; END IF;
 SELECT sum((p.value->>'discount_minor')::numeric) INTO line_discount
  FROM jsonb_array_elements(q.snapshot->'lines') p(value);
 IF line_discount IS DISTINCT FROM NEW.discount_minor::numeric THEN
  RAISE EXCEPTION 'Redemption requires exact allocated discount' USING ERRCODE='23514'; END IF;
 limits=draft.policy->'limits'; promotion=draft.policy->'promotion';
 IF jsonb_typeof(limits) IS DISTINCT FROM 'object'
  OR NOT (limits ? 'per_customer_redemptions')
  OR (limits->'per_customer_redemptions' IS DISTINCT FROM 'null'::jsonb AND
   (q.promotion_account_id IS NULL
    OR jsonb_typeof(limits->'per_customer_redemptions') IS DISTINCT FROM 'number'
    OR coalesce(limits->>'per_customer_redemptions','') !~ '^[1-9][0-9]{0,6}$'))
  OR jsonb_typeof(limits->'total_redemptions') IS DISTINCT FROM 'number'
  OR jsonb_typeof(limits->'total_discount_minor') IS DISTINCT FROM 'number'
  OR coalesce(limits->>'total_redemptions','') !~ '^[1-9][0-9]{0,15}$'
  OR coalesce(limits->>'total_discount_minor','') !~ '^[1-9][0-9]{0,15}$'
  OR coalesce(promotion->>'valid_from','') !~ '^(0|[1-9][0-9]{0,15})$'
  OR coalesce(promotion->>'expires_at','') !~ '^(0|[1-9][0-9]{0,15})$' THEN
  RAISE EXCEPTION 'Redemption requires bounded guest limits' USING ERRCODE='23514'; END IF;
 IF limits->'per_customer_redemptions' IS DISTINCT FROM 'null'::jsonb
  AND (limits->>'per_customer_redemptions')::numeric>(limits->>'total_redemptions')::numeric THEN
  RAISE EXCEPTION 'Redemption requires bounded per-account limit' USING ERRCODE='23514'; END IF;
 IF (limits->>'total_redemptions')::numeric>1000000
  OR (limits->>'total_discount_minor')::numeric>9007199254740991
  OR (promotion->>'valid_from')::numeric>NEW.created_at
  OR (promotion->>'expires_at')::numeric<=NEW.created_at THEN
  RAISE EXCEPTION 'Redemption outside policy terms' USING ERRCODE='23514'; END IF;
 -- Reviewed terms bind a redemption even if a private SQL writer constructs
 -- an internally coherent but falsely discounted version-2 quote. Numeric
 -- intermediates stay bounded by at most twenty safe-integer source lines.
 IF jsonb_typeof(promotion->'products') IS DISTINCT FROM 'array'
  OR jsonb_typeof(promotion->'discount') IS DISTINCT FROM 'object'
  OR jsonb_typeof(promotion->'minimum_eligible_gross_minor') IS DISTINCT FROM 'number'
  OR jsonb_typeof(promotion->'maximum_discount_minor') IS DISTINCT FROM 'number'
  OR coalesce(promotion->>'minimum_eligible_gross_minor','') !~ '^(0|[1-9][0-9]{0,15})$'
  OR coalesce(promotion->>'maximum_discount_minor','') !~ '^[1-9][0-9]{0,15}$'
  OR (promotion->'discount'->>'kind'='fixed' AND
   (jsonb_typeof(promotion->'discount'->'minor_units') IS DISTINCT FROM 'number'
    OR coalesce(promotion->'discount'->>'minor_units','') !~ '^[1-9][0-9]{0,15}$'))
  OR (promotion->'discount'->>'kind'='percentage' AND
   (jsonb_typeof(promotion->'discount'->'basis_points') IS DISTINCT FROM 'number'
    OR coalesce(promotion->'discount'->>'basis_points','') !~ '^[1-9][0-9]{0,4}$'))
  OR coalesce(promotion->'discount'->>'kind','') NOT IN ('fixed','percentage') THEN
  RAISE EXCEPTION 'Redemption requires bounded reviewed terms' USING ERRCODE='23514'; END IF;
 IF jsonb_array_length(promotion->'products') NOT BETWEEN 1 AND 20
  OR (promotion->>'minimum_eligible_gross_minor')::numeric>9007199254740991
  OR (promotion->>'maximum_discount_minor')::numeric>9007199254740991
  OR (promotion->'discount'->>'kind'='fixed' AND (promotion->'discount'->>'minor_units')::numeric>9007199254740991)
  OR (promotion->'discount'->>'kind'='percentage' AND (promotion->'discount'->>'basis_points')::numeric>10000) THEN
  RAISE EXCEPTION 'Redemption requires bounded reviewed terms' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(promotion->'products') p(value)
  WHERE jsonb_typeof(p.value) IS DISTINCT FROM 'object'
   OR jsonb_typeof(p.value->'sku') IS DISTINCT FROM 'string'
   OR jsonb_typeof(p.value->'publication_revision') IS DISTINCT FROM 'number')
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(promotion->'products') p(value)
   GROUP BY p.value->>'sku' HAVING count(*)>1)
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(q.snapshot->'original'->'lines') l(value)
   JOIN jsonb_array_elements(promotion->'products') p(value) ON p.value->'sku'=l.value->'sku'
   WHERE p.value->'publication_revision' IS DISTINCT FROM l.value->'publication_revision') THEN
  RAISE EXCEPTION 'Redemption requires exact reviewed product scope' USING ERRCODE='23514'; END IF;
 SELECT coalesce(sum((l.value->'amounts'->>'gross_minor')::numeric),0) INTO eligible_gross
  FROM jsonb_array_elements(q.snapshot->'original'->'lines') l(value)
  WHERE EXISTS(SELECT 1 FROM jsonb_array_elements(promotion->'products') p(value)
   WHERE p.value->'sku'=l.value->'sku' AND p.value->'publication_revision'=l.value->'publication_revision');
 IF eligible_gross<=0 OR eligible_gross>9007199254740991
  OR eligible_gross<(promotion->>'minimum_eligible_gross_minor')::numeric THEN
  RAISE EXCEPTION 'Redemption requires eligible reviewed subtotal' USING ERRCODE='23514'; END IF;
 requested_discount=CASE WHEN promotion->'discount'->>'kind'='fixed'
  THEN (promotion->'discount'->>'minor_units')::numeric
  ELSE div(eligible_gross*(promotion->'discount'->>'basis_points')::numeric+5000,10000) END;
 IF NEW.discount_minor IS DISTINCT FROM least(requested_discount,
   (promotion->>'maximum_discount_minor')::numeric,eligible_gross) THEN
  RAISE EXCEPTION 'Redemption requires exact reviewed discount' USING ERRCODE='23514'; END IF;
 -- Largest remainder, with bytewise SKU tie-break, matches the domain allocator.
 IF EXISTS(WITH candidates AS (
   SELECT l.value->>'sku' AS sku, p.value AS discounted,
    (l.value->'amounts'->>'gross_minor')::numeric AS gross,
    EXISTS(SELECT 1 FROM jsonb_array_elements(promotion->'products') e(value)
     WHERE e.value->'sku'=l.value->'sku' AND e.value->'publication_revision'=l.value->'publication_revision') AS eligible
   FROM jsonb_array_elements(q.snapshot->'original'->'lines') WITH ORDINALITY l(value,n)
   JOIN jsonb_array_elements(q.snapshot->'lines') WITH ORDINALITY p(value,n) USING(n)
  ), parts AS (
   SELECT *,CASE WHEN eligible THEN div(NEW.discount_minor::numeric*gross,eligible_gross) ELSE 0 END AS base,
    CASE WHEN eligible THEN mod(NEW.discount_minor::numeric*gross,eligible_gross) ELSE -1 END AS remainder
   FROM candidates
  ), ranked AS (
   SELECT *,row_number() OVER (ORDER BY remainder DESC,sku COLLATE "C") AS rank,
    sum(base) OVER () AS base_total FROM parts
  ) SELECT 1 FROM ranked WHERE (discounted->>'discount_minor')::numeric <>
   base+CASE WHEN eligible AND rank<=NEW.discount_minor::numeric-base_total THEN 1 ELSE 0 END) THEN
  RAISE EXCEPTION 'Redemption requires canonical discount allocation' USING ERRCODE='23514'; END IF;
 -- The discounted line VAT and total are derived from the retained original
 -- rates and unchanged shipping, using the same half-up integer rule.
 IF q.snapshot->'shipping' IS DISTINCT FROM q.snapshot->'original'->'shipping'
  OR NOT promoted_quote_amount_shape(q.snapshot->'shipping')
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(q.snapshot->'original'->'lines') l(value)
   WHERE jsonb_typeof(l.value->'vat_basis_points') IS DISTINCT FROM 'number'
    OR coalesce(l.value->>'vat_basis_points','') !~ '^(0|[1-9][0-9]{0,4})$') THEN
  RAISE EXCEPTION 'Redemption requires original tax and shipping source' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(q.snapshot->'original'->'lines') WITH ORDINALITY l(value,n)
  JOIN jsonb_array_elements(q.snapshot->'lines') WITH ORDINALITY p(value,n) USING(n)
  WHERE (l.value->>'vat_basis_points')::numeric>10000
   OR (p.value->'amounts'->>'vat_minor')::numeric <>
    div((p.value->'amounts'->>'gross_minor')::numeric*(l.value->>'vat_basis_points')::numeric
      +div(10000+(l.value->>'vat_basis_points')::numeric,2),
      10000+(l.value->>'vat_basis_points')::numeric)) THEN
  RAISE EXCEPTION 'Redemption requires exact discounted VAT' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM (VALUES ('gross_minor'),('net_minor'),('vat_minor')) k(name)
  WHERE (q.snapshot->'total'->>k.name)::numeric <>
   (q.snapshot->'shipping'->>k.name)::numeric +
   (SELECT sum((p.value->'amounts'->>k.name)::numeric) FROM jsonb_array_elements(q.snapshot->'lines') p(value))) THEN
  RAISE EXCEPTION 'Redemption requires exact discounted total' USING ERRCODE='23514'; END IF;
 -- The immutable predecessor is found by the unique promotion/sequence index.
 -- The anchor claim serializes writers, so this is the latest lifetime receipt
 -- even across revisions, with no scan of the promotion's redemption history.
 SELECT redemption_number,lifetime_discount_minor INTO previous_count,previous_discount
  FROM promotion_redemptions WHERE promotion_id=NEW.promotion_id
  ORDER BY redemption_number DESC LIMIT 1;
 previous_count=coalesce(previous_count,0); previous_discount=coalesce(previous_discount,0);
 IF previous_count+1>(limits->>'total_redemptions')::numeric
  OR previous_discount::numeric+NEW.discount_minor>(limits->>'total_discount_minor')::numeric THEN
  RAISE EXCEPTION 'Promotion lifetime limit exhausted' USING ERRCODE='23514'; END IF;
 -- Anonymous history cannot safely be attributed to any account. Once a
 -- cap is enabled, fail closed across all revisions if any such debit exists.
 IF limits->'per_customer_redemptions' IS DISTINCT FROM 'null'::jsonb
  AND EXISTS (SELECT 1 FROM promotion_redemptions WHERE promotion_id=NEW.promotion_id
   AND promotion_account_id IS NULL) THEN
  RAISE EXCEPTION 'Capped promotion has anonymous redemption history' USING ERRCODE='23514'; END IF;
 IF NEW.promotion_account_id IS NULL THEN
  NEW.account_redemption_number=NULL;
 ELSE
  SELECT account_redemption_number INTO account_count FROM promotion_redemptions
   WHERE promotion_id=NEW.promotion_id AND promotion_account_id=NEW.promotion_account_id
   ORDER BY account_redemption_number DESC LIMIT 1;
  account_count=coalesce(account_count,0)+1;
  IF limits->'per_customer_redemptions' IS DISTINCT FROM 'null'::jsonb
   AND account_count>(limits->>'per_customer_redemptions')::numeric THEN
   RAISE EXCEPTION 'Promotion account limit exhausted' USING ERRCODE='23514'; END IF;
  NEW.account_redemption_number=account_count;
 END IF;
 NEW.redemption_number=previous_count+1;
 NEW.lifetime_discount_minor=previous_discount+NEW.discount_minor;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END;
$$;
CREATE OR REPLACE FUNCTION require_checkout_redemption() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE q guest_quote_snapshots%ROWTYPE;
BEGIN
 SELECT * INTO q FROM guest_quote_snapshots WHERE id=NEW.quote_id;
 IF (q.promotion_id IS NULL AND EXISTS(SELECT 1 FROM promotion_redemptions WHERE order_id=NEW.id))
  OR (q.promotion_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM promotion_redemptions r
   WHERE r.order_id=NEW.id AND r.quote_id=NEW.quote_id AND r.promotion_id=q.promotion_id
    AND r.promotion_revision=q.promotion_revision AND r.promotion_review_revision=q.promotion_review_revision
    AND r.promotion_account_id IS NOT DISTINCT FROM q.promotion_account_id
    AND r.transaction_id=NEW.promotion_redemption_transaction_id)) THEN
  RAISE EXCEPTION 'Checkout requires its exact promotion redemption' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
-- Existing payment and link adapters only prove guest custody. They cannot
-- prepare payment or issue a guest-link proof for an account-bound order.
CREATE FUNCTION guard_account_order_guest_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS (SELECT 1 FROM checkout_orders o JOIN guest_quote_snapshots q ON q.id=o.quote_id
  WHERE o.id=NEW.order_id AND q.promotion_account_id IS NOT NULL) THEN
  RAISE EXCEPTION 'Account-bound order requires account-aware custody' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_account_order_guest_only BEFORE INSERT ON payment_attempts
 FOR EACH ROW EXECUTE FUNCTION guard_account_order_guest_only();
CREATE TRIGGER aa_account_order_guest_only BEFORE INSERT ON guest_order_link_proofs
 FOR EACH ROW EXECUTE FUNCTION guard_account_order_guest_only();
