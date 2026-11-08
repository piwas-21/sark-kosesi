-- Atomic guest promotion redemptions, with no payment or release authority.
CREATE OR REPLACE FUNCTION guard_checkout_order() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE quote guest_quote_snapshots%ROWTYPE; original_market jsonb; members jsonb; selected jsonb; source jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Order snapshots are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO quote FROM guest_quote_snapshots WHERE id=NEW.quote_id AND basket_id=NEW.basket_id AND basket_revision=NEW.basket_revision;
 source=CASE WHEN quote.promotion_id IS NULL THEN quote.snapshot ELSE quote.snapshot->'original' END;
 SELECT policy INTO original_market FROM market_policy_drafts WHERE id=quote.policy_id AND revision=quote.policy_revision;
 IF quote.id IS NULL OR original_market IS NULL
  OR NEW.snapshot->'schema_version' IS DISTINCT FROM (CASE WHEN quote.promotion_id IS NULL THEN '1'::jsonb ELSE '2'::jsonb END)
  OR NEW.snapshot->'identity' IS DISTINCT FROM jsonb_build_object('id',NEW.id,'basket_id',NEW.basket_id,'quote_id',NEW.quote_id,'stock_group_id',NEW.stock_group_id,'created_at',NEW.created_at,'hold_expires_at',NEW.hold_expires_at)
  OR NEW.snapshot->'quote' IS DISTINCT FROM quote.snapshot OR NEW.snapshot->'market' IS DISTINCT FROM original_market
  OR NEW.intent->>'quote_id' IS DISTINCT FROM NEW.quote_id OR NEW.intent->>'expected_basket_revision' IS DISTINCT FROM NEW.basket_revision::text
  OR NEW.intent->'confirmed' IS DISTINCT FROM 'true'::jsonb
  OR NEW.intent->'buyer' IS DISTINCT FROM NEW.snapshot->'buyer'
  OR NEW.intent->'shipping_address' IS DISTINCT FROM NEW.snapshot->'shipping_address'
  OR NEW.intent->'billing_address' IS DISTINCT FROM NEW.snapshot->'billing_address'
  OR NEW.intent->>'accepted_terms_version' IS DISTINCT FROM NEW.snapshot->'seller'->>'terms_version'
  OR jsonb_typeof(NEW.snapshot->'products') IS DISTINCT FROM 'array' THEN
  RAISE EXCEPTION 'Order requires exact owned quote, intent and source snapshots' USING ERRCODE='23514';
 END IF;
 IF NOT EXISTS(SELECT 1 FROM basket_hold_groups g WHERE g.id=NEW.stock_group_id AND g.owner_id=NEW.id AND g.created_at=NEW.created_at AND g.expires_at=NEW.hold_expires_at AND g.sealed AND g.members_count=jsonb_array_length(source->'lines')) THEN
  RAISE EXCEPTION 'Order requires complete owned initial reservation' USING ERRCODE='23514';
 END IF;
 SELECT jsonb_agg(jsonb_build_object('sku',m.sku,'bom_revision',m.revision,'quantity',m.units) ORDER BY m.sku COLLATE "C") INTO members FROM basket_hold_members m WHERE m.group_id=NEW.stock_group_id;
 SELECT jsonb_agg(jsonb_build_object('sku',l->>'sku','bom_revision',l->'bom_revision','quantity',l->'quantity') ORDER BY (l->>'sku') COLLATE "C") INTO selected FROM jsonb_array_elements(source->'lines') l;
 IF members IS DISTINCT FROM selected THEN RAISE EXCEPTION 'Order reservations require exact quote membership' USING ERRCODE='23514'; END IF;
 SELECT jsonb_agg(jsonb_build_object('sku',p->>'sku','bom_revision',p->'bom_revision','publication_revision',p->'publication_revision') ORDER BY (p->>'sku') COLLATE "C") INTO members FROM jsonb_array_elements(NEW.snapshot->'products') p;
 SELECT jsonb_agg(jsonb_build_object('sku',l->>'sku','bom_revision',l->'bom_revision','publication_revision',l->'publication_revision') ORDER BY (l->>'sku') COLLATE "C") INTO selected FROM jsonb_array_elements(source->'lines') l;
 IF members IS DISTINCT FROM selected THEN RAISE EXCEPTION 'Order merchandise requires exact immutable sources' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;

-- Existing orders remain legacy records. Only new promoted orders acquire a
-- transaction marker; a separately committed order cannot acquire a debit.
ALTER TABLE checkout_orders ADD COLUMN promotion_redemption_transaction_id xid8;
DROP TRIGGER aa_checkout_unpromoted_quote ON checkout_orders;
CREATE FUNCTION guard_checkout_promotion_kind() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE q guest_quote_snapshots%ROWTYPE;
BEGIN
 SELECT * INTO q FROM guest_quote_snapshots WHERE id=NEW.quote_id;
 IF q.id IS NULL THEN RAISE EXCEPTION 'Order requires quote' USING ERRCODE='23514'; END IF;
 IF q.promotion_id IS NULL THEN
  IF q.promotion_revision IS NOT NULL OR q.promotion_review_revision IS NOT NULL
   OR q.snapshot->'schema_version' IS DISTINCT FROM '1'::jsonb
   OR NEW.promotion_redemption_transaction_id IS NOT NULL THEN
   RAISE EXCEPTION 'Legacy order requires a legacy quote' USING ERRCODE='23514'; END IF;
 ELSE
  IF q.promotion_revision IS NULL OR q.promotion_review_revision IS NULL
   OR q.snapshot->'schema_version' IS DISTINCT FROM '2'::jsonb THEN
   RAISE EXCEPTION 'Promoted order requires a promoted quote' USING ERRCODE='23514'; END IF;
  NEW.promotion_redemption_transaction_id=pg_current_xact_id();
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_checkout_promotion_kind BEFORE INSERT ON checkout_orders
 FOR EACH ROW EXECUTE FUNCTION guard_checkout_promotion_kind();

CREATE TABLE promotion_redemptions (
 order_id text PRIMARY KEY REFERENCES checkout_orders(id),
 quote_id text UNIQUE NOT NULL REFERENCES guest_quote_snapshots(id),
 promotion_id text NOT NULL,
 promotion_revision integer NOT NULL,
 promotion_review_revision integer NOT NULL,
 discount_minor bigint NOT NULL CHECK(discount_minor BETWEEN 1 AND 9007199254740991),
 redemption_number bigint NOT NULL CHECK(redemption_number BETWEEN 1 AND 1000000),
 lifetime_discount_minor bigint NOT NULL CHECK(lifetime_discount_minor BETWEEN 1 AND 9007199254740991),
 created_at bigint NOT NULL CHECK(created_at BETWEEN 0 AND 9007199254740991),
 transaction_id xid8 NOT NULL,
 UNIQUE(promotion_id,redemption_number),
 FOREIGN KEY(promotion_id,promotion_revision,promotion_review_revision)
  REFERENCES promotion_policy_reviews(id,draft_revision,revision)
);
CREATE INDEX promotion_redemptions_transaction ON promotion_redemptions(promotion_id,transaction_id);

-- Reconstruct retained original amounts from immutable publication and market
-- sources. This predicate does not confer current market/publication readiness.
CREATE FUNCTION promoted_redemption_original_source(q guest_quote_snapshots) RETURNS boolean
 LANGUAGE plpgsql STABLE AS $$
DECLARE source jsonb; market jsonb; shipping jsonb; item jsonb; rule jsonb; original_intent jsonb;
 rate numeric; gross numeric; vat numeric; total_gross numeric; total_net numeric; total_vat numeric;
 publication_choice jsonb; revision_value numeric; publication_revision integer; price_revision integer;
BEGIN
 source=q.snapshot->'original';
 SELECT policy INTO market FROM market_policy_drafts WHERE id=q.policy_id AND revision=q.policy_revision;
 IF market IS NULL OR jsonb_typeof(source->'lines') IS DISTINCT FROM 'array'
  OR jsonb_typeof(market->'products') IS DISTINCT FROM 'array'
  OR jsonb_array_length(source->'lines') NOT BETWEEN 1 AND 20
  OR jsonb_array_length(market->'products') NOT BETWEEN 1 AND 20
  OR NOT promoted_quote_amount_shape(source->'total')
  OR NOT promoted_quote_amount_shape(source->'shipping')
  OR source->'binding' IS DISTINCT FROM jsonb_build_object('quote_id',q.id,'basket_id',q.basket_id,
   'basket_revision',q.basket_revision,'locale',q.locale,'country',q.country,'postcode',q.postcode,
   'policy_id',q.policy_id,'policy_revision',q.policy_revision,'policy_review_revision',q.policy_review_revision,
   'created_at',q.created_at,'expires_at',q.expires_at)
  OR source->'schema_version' IS DISTINCT FROM '1'::jsonb
  OR source->'currency' IS DISTINCT FROM '"EUR"'::jsonb THEN RETURN false; END IF;
 SELECT jsonb_agg(jsonb_build_object('sku',l.value->>'sku','quantity',l.value->'quantity')
  ORDER BY (l.value->>'sku') COLLATE "C") INTO original_intent
  FROM jsonb_array_elements(source->'lines') l(value);
 IF original_intent IS DISTINCT FROM (SELECT lines FROM guest_basket_effects
  WHERE basket_id=q.basket_id AND revision=q.basket_revision) THEN RETURN false; END IF;
 shipping=market->'shipping';
 IF jsonb_typeof(shipping->'gross'->'minor_units') IS DISTINCT FROM 'number'
  OR coalesce(shipping->'gross'->>'minor_units','') !~ '^(0|[1-9][0-9]{0,15})$'
  OR shipping->'gross'->'currency' IS DISTINCT FROM '"EUR"'::jsonb
  OR jsonb_typeof(shipping->'vat_basis_points') IS DISTINCT FROM 'number'
  OR coalesce(shipping->>'vat_basis_points','') !~ '^(0|[1-9][0-9]{0,4})$'
  OR source->'shipping_vat_basis_points' IS DISTINCT FROM shipping->'vat_basis_points'
  OR source->'shipping'->'gross_minor' IS DISTINCT FROM shipping->'gross'->'minor_units' THEN RETURN false; END IF;
 rate=(shipping->>'vat_basis_points')::numeric;
 gross=(shipping->'gross'->>'minor_units')::numeric;
 IF rate>10000 OR gross>9007199254740991 THEN RETURN false; END IF;
 vat=div(gross*rate+div(10000+rate,2),10000+rate);
 IF (source->'shipping'->>'vat_minor')::numeric<>vat
  OR (source->'shipping'->>'net_minor')::numeric<>gross-vat THEN RETURN false; END IF;
 total_gross=gross; total_vat=vat; total_net=gross-vat;
 FOR item IN SELECT value FROM jsonb_array_elements(source->'lines') LOOP
  IF jsonb_typeof(item) IS DISTINCT FROM 'object'
   OR jsonb_typeof(item->'sku') IS DISTINCT FROM 'string'
   OR jsonb_typeof(item->'quantity') IS DISTINCT FROM 'number'
   OR coalesce(item->>'quantity','') !~ '^[1-9][0-9]{0,2}$'
   OR jsonb_typeof(item->'unit_gross_minor') IS DISTINCT FROM 'number'
   OR coalesce(item->>'unit_gross_minor','') !~ '^(0|[1-9][0-9]{0,15})$'
   OR jsonb_typeof(item->'vat_basis_points') IS DISTINCT FROM 'number'
   OR coalesce(item->>'vat_basis_points','') !~ '^(0|[1-9][0-9]{0,4})$'
   OR NOT promoted_quote_amount_shape(item->'amounts') THEN RETURN false; END IF;
  IF (item->>'quantity')::numeric>100 OR (item->>'unit_gross_minor')::numeric>9007199254740991
   OR (item->>'vat_basis_points')::numeric>10000 THEN RETURN false; END IF;
  -- Validate before casting in separate statements: no planner predicate ordering
  -- can turn malformed or out-of-range JSON into a failing integer cast.
  IF jsonb_typeof(item->'publication_revision') IS DISTINCT FROM 'number' THEN RETURN false; END IF;
  revision_value=(item->>'publication_revision')::numeric;
  IF revision_value<1 OR revision_value>2147483647 OR revision_value<>trunc(revision_value) THEN RETURN false; END IF;
  publication_revision=revision_value::integer;
  SELECT p.choice INTO publication_choice FROM catalogue_publication_revisions p
   WHERE p.sku=item->>'sku' AND p.locale=q.locale AND p.revision=publication_revision
    AND p.choice->>'action'='activate' AND p.choice->'selection'->'facts'->'bom_revision'=item->'bom_revision';
  IF NOT FOUND OR jsonb_typeof(publication_choice->'selection'->'facts'->'price_revision') IS DISTINCT FROM 'number' THEN RETURN false; END IF;
  revision_value=(publication_choice->'selection'->'facts'->>'price_revision')::numeric;
  IF revision_value<1 OR revision_value>2147483647 OR revision_value<>trunc(revision_value) THEN RETURN false; END IF;
  price_revision=revision_value::integer;
  IF NOT EXISTS(SELECT 1 FROM catalogue_price_revisions price
   WHERE price.sku=item->>'sku' AND price.revision=price_revision
    AND price.currency='EUR' AND to_jsonb(price.minor_units)=item->'unit_gross_minor') THEN RETURN false; END IF;
  IF (SELECT count(*) FROM jsonb_array_elements(market->'products') r(value)
   WHERE r.value->'sku'=item->'sku')<>1 THEN RETURN false; END IF;
  SELECT value INTO rule FROM jsonb_array_elements(market->'products') r(value) WHERE r.value->'sku'=item->'sku';
  IF rule->'publication_revision' IS DISTINCT FROM item->'publication_revision'
   OR rule->'vat_basis_points' IS DISTINCT FROM item->'vat_basis_points'
   OR jsonb_typeof(rule->'max_quantity') IS DISTINCT FROM 'number'
   OR coalesce(rule->>'max_quantity','') !~ '^[1-9][0-9]{0,2}$' THEN RETURN false; END IF;
  IF (rule->>'max_quantity')::numeric>100 OR (item->>'quantity')::numeric>(rule->>'max_quantity')::numeric THEN RETURN false; END IF;
  rate=(item->>'vat_basis_points')::numeric;
  gross=(item->>'unit_gross_minor')::numeric*(item->>'quantity')::numeric;
  IF gross>9007199254740991 OR (item->'amounts'->>'gross_minor')::numeric<>gross THEN RETURN false; END IF;
  vat=div(gross*rate+div(10000+rate,2),10000+rate);
  IF (item->'amounts'->>'vat_minor')::numeric<>vat
   OR (item->'amounts'->>'net_minor')::numeric<>gross-vat THEN RETURN false; END IF;
  total_gross=total_gross+gross; total_vat=total_vat+vat; total_net=total_net+gross-vat;
 END LOOP;
 RETURN total_gross<=9007199254740991 AND total_net<=9007199254740991 AND total_vat<=9007199254740991
  AND (source->'total'->>'gross_minor')::numeric=total_gross
  AND (source->'total'->>'net_minor')::numeric=total_net
  AND (source->'total'->>'vat_minor')::numeric=total_vat;
END;
$$;

CREATE FUNCTION guard_promotion_redemption() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE q guest_quote_snapshots%ROWTYPE; o checkout_orders%ROWTYPE;
 draft promotion_policy_drafts%ROWTYPE; review promotion_policy_reviews%ROWTYPE;
 limits jsonb; promotion jsonb; previous_count bigint; previous_discount bigint;
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
  OR limits->'per_customer_redemptions' IS DISTINCT FROM 'null'::jsonb
  OR jsonb_typeof(limits->'total_redemptions') IS DISTINCT FROM 'number'
  OR jsonb_typeof(limits->'total_discount_minor') IS DISTINCT FROM 'number'
  OR coalesce(limits->>'total_redemptions','') !~ '^[1-9][0-9]{0,15}$'
  OR coalesce(limits->>'total_discount_minor','') !~ '^[1-9][0-9]{0,15}$'
  OR coalesce(promotion->>'valid_from','') !~ '^(0|[1-9][0-9]{0,15})$'
  OR coalesce(promotion->>'expires_at','') !~ '^(0|[1-9][0-9]{0,15})$' THEN
  RAISE EXCEPTION 'Redemption requires bounded guest limits' USING ERRCODE='23514'; END IF;
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
 NEW.redemption_number=previous_count+1;
 NEW.lifetime_discount_minor=previous_discount+NEW.discount_minor;
 NEW.transaction_id=pg_current_xact_id();
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_promotion_redemption BEFORE INSERT OR UPDATE OR DELETE ON promotion_redemptions
 FOR EACH ROW EXECUTE FUNCTION guard_promotion_redemption();

CREATE FUNCTION require_checkout_redemption() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE q guest_quote_snapshots%ROWTYPE;
BEGIN
 SELECT * INTO q FROM guest_quote_snapshots WHERE id=NEW.quote_id;
 IF (q.promotion_id IS NULL AND EXISTS(SELECT 1 FROM promotion_redemptions WHERE order_id=NEW.id))
  OR (q.promotion_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM promotion_redemptions r
   WHERE r.order_id=NEW.id AND r.quote_id=NEW.quote_id AND r.promotion_id=q.promotion_id
    AND r.promotion_revision=q.promotion_revision AND r.promotion_review_revision=q.promotion_review_revision
    AND r.transaction_id=NEW.promotion_redemption_transaction_id)) THEN
  RAISE EXCEPTION 'Checkout requires its exact promotion redemption' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_checkout_redemption AFTER INSERT ON checkout_orders
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_checkout_redemption();

-- A redemption also seals its promotion source against same-transaction edits.
CREATE OR REPLACE FUNCTION guard_promotion_quote_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM guest_quote_snapshots q WHERE q.promotion_id=NEW.id
  AND q.promotion_transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Promoted quote source is sealed for this transaction' USING ERRCODE='23514'; END IF;
 IF EXISTS(SELECT 1 FROM promotion_redemptions r WHERE r.promotion_id=NEW.id
  AND r.transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Promotion redemption source is sealed for this transaction' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
