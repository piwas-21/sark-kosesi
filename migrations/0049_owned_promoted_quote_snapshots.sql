-- Private indicative snapshots only. No coupon redemption or order authority.
ALTER TABLE guest_quote_snapshots
 ADD COLUMN promotion_id text,
 ADD COLUMN promotion_revision integer,
 ADD COLUMN promotion_review_revision integer,
 ADD COLUMN promotion_transaction_id xid8,
 ADD CONSTRAINT guest_quote_promotion_all_or_none CHECK (
  (num_nonnulls(promotion_id,promotion_revision,promotion_review_revision,promotion_transaction_id)=0)
  OR (num_nonnulls(promotion_id,promotion_revision,promotion_review_revision,promotion_transaction_id)=4)
 ),
 ADD CONSTRAINT guest_quote_promotion_review_fk FOREIGN KEY
  (promotion_id,promotion_revision,promotion_review_revision)
  REFERENCES promotion_policy_reviews(id,draft_revision,revision);
CREATE INDEX guest_quote_promotion_transaction ON guest_quote_snapshots(promotion_id,promotion_transaction_id)
 WHERE promotion_id IS NOT NULL;

-- A closed minor-unit object is shape evidence only; Rust independently
-- reconstructs the promotion arithmetic before accepting an indicative quote.
CREATE FUNCTION promoted_quote_amount_shape(value jsonb) RETURNS boolean
 LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE gross numeric; net numeric; vat numeric;
BEGIN
 IF jsonb_typeof(value) IS DISTINCT FROM 'object'
  OR value - ARRAY['gross_minor','net_minor','vat_minor'] IS DISTINCT FROM '{}'::jsonb
  OR NOT value ?& ARRAY['gross_minor','net_minor','vat_minor']
  OR jsonb_typeof(value->'gross_minor') IS DISTINCT FROM 'number'
  OR jsonb_typeof(value->'net_minor') IS DISTINCT FROM 'number'
  OR jsonb_typeof(value->'vat_minor') IS DISTINCT FROM 'number' THEN RETURN false; END IF;
 gross=(value->>'gross_minor')::numeric; net=(value->>'net_minor')::numeric; vat=(value->>'vat_minor')::numeric;
 RETURN gross BETWEEN 0 AND 9007199254740991 AND net BETWEEN 0 AND gross
  AND vat BETWEEN 0 AND gross AND trunc(gross)=gross AND trunc(net)=net
  AND trunc(vat)=vat AND net+vat=gross;
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range THEN RETURN false;
END;
$$;

CREATE OR REPLACE FUNCTION guard_guest_quote() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE binding jsonb; intent jsonb; original jsonb; item jsonb; source jsonb; promoted jsonb;
 draft promotion_policy_drafts%ROWTYPE; review promotion_policy_reviews%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Quote history is immutable' USING ERRCODE='23514'; END IF;
 IF NEW.promotion_id IS NULL AND NEW.promotion_revision IS NULL AND NEW.promotion_review_revision IS NULL THEN
  IF NEW.promotion_transaction_id IS NOT NULL THEN
   RAISE EXCEPTION 'Legacy quote has no promotion transaction' USING ERRCODE='23514'; END IF;
  source=NEW.snapshot;
 ELSIF NEW.promotion_id IS NOT NULL AND NEW.promotion_revision IS NOT NULL AND NEW.promotion_review_revision IS NOT NULL THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Promoted quote requires read committed' USING ERRCODE='23514'; END IF;
  -- Lock before separate latest-source reads; a writer's no-op anchor update
  -- serializes concurrent withdrawal and draft changes through this transaction.
  PERFORM 1 FROM promotion_policy_anchors WHERE id=NEW.promotion_id FOR SHARE;
  IF NOT FOUND THEN RAISE EXCEPTION 'Promoted quote requires a promotion anchor' USING ERRCODE='23514'; END IF;
  SELECT * INTO draft FROM promotion_policy_drafts WHERE id=NEW.promotion_id ORDER BY revision DESC LIMIT 1;
  SELECT * INTO review FROM promotion_policy_reviews WHERE id=NEW.promotion_id AND draft_revision=NEW.promotion_revision ORDER BY revision DESC LIMIT 1;
  IF draft.id IS NULL OR review.id IS NULL OR draft.revision IS DISTINCT FROM NEW.promotion_revision
   OR review.revision IS DISTINCT FROM NEW.promotion_review_revision OR review.decision IS DISTINCT FROM 'approved'
   OR review.actor IS NOT DISTINCT FROM draft.actor OR draft.policy->'currency' IS DISTINCT FROM '"EUR"'::jsonb
   OR draft.policy->'promotion'->>'id' IS DISTINCT FROM NEW.promotion_id
   OR draft.policy->'promotion'->'revision' IS DISTINCT FROM to_jsonb(NEW.promotion_revision)
   OR jsonb_typeof(draft.policy->'promotion'->'countries') IS DISTINCT FROM 'array'
   OR NOT (draft.policy->'promotion'->'countries' @> jsonb_build_array(NEW.country))
   OR jsonb_typeof(draft.policy->'limits') IS DISTINCT FROM 'object'
   OR NOT (draft.policy->'limits' ? 'per_customer_redemptions')
   OR draft.policy->'limits'->'per_customer_redemptions' IS DISTINCT FROM 'null'::jsonb
   OR jsonb_typeof(draft.policy->'promotion'->'valid_from') IS DISTINCT FROM 'number'
   OR jsonb_typeof(draft.policy->'promotion'->'expires_at') IS DISTINCT FROM 'number'
   OR coalesce(draft.policy->'promotion'->>'valid_from','') !~ '^(0|[1-9][0-9]{0,15})$'
   OR coalesce(draft.policy->'promotion'->>'expires_at','') !~ '^(0|[1-9][0-9]{0,15})$' THEN
   RAISE EXCEPTION 'Promoted quote requires current independent guest policy' USING ERRCODE='23514'; END IF;
  -- Reject malformed/extra source fields even when an imported draft was approved.
  IF jsonb_typeof(draft.policy) IS DISTINCT FROM 'object'
   OR draft.policy - ARRAY['currency','promotion','limits'] IS DISTINCT FROM '{}'::jsonb
   OR NOT draft.policy ?& ARRAY['currency','promotion','limits']
   OR jsonb_typeof(draft.policy->'promotion') IS DISTINCT FROM 'object'
   OR (draft.policy->'promotion') - ARRAY['id','revision','valid_from','expires_at','countries','products','minimum_eligible_gross_minor','maximum_discount_minor','discount','evidence_ref'] IS DISTINCT FROM '{}'::jsonb
   OR NOT draft.policy->'promotion' ?& ARRAY['id','revision','valid_from','expires_at','countries','products','minimum_eligible_gross_minor','maximum_discount_minor','discount','evidence_ref']
   OR (draft.policy->'limits') - ARRAY['total_discount_minor','total_redemptions','per_customer_redemptions'] IS DISTINCT FROM '{}'::jsonb
   OR NOT draft.policy->'limits' ?& ARRAY['total_discount_minor','total_redemptions','per_customer_redemptions']
   OR jsonb_typeof(draft.policy->'promotion'->'products') IS DISTINCT FROM 'array'
   OR jsonb_typeof(draft.policy->'promotion'->'discount') IS DISTINCT FROM 'object'
   OR jsonb_typeof(draft.policy->'promotion'->'evidence_ref') IS DISTINCT FROM 'string'
   OR jsonb_typeof(draft.policy->'promotion'->'minimum_eligible_gross_minor') IS DISTINCT FROM 'number'
   OR jsonb_typeof(draft.policy->'promotion'->'maximum_discount_minor') IS DISTINCT FROM 'number'
   OR jsonb_typeof(draft.policy->'limits'->'total_discount_minor') IS DISTINCT FROM 'number'
   OR jsonb_typeof(draft.policy->'limits'->'total_redemptions') IS DISTINCT FROM 'number' THEN
   RAISE EXCEPTION 'Promoted quote requires closed policy source' USING ERRCODE='23514'; END IF;
  IF jsonb_array_length(draft.policy->'promotion'->'countries') NOT BETWEEN 1 AND 27
   OR jsonb_array_length(draft.policy->'promotion'->'products') NOT BETWEEN 1 AND 20 THEN
   RAISE EXCEPTION 'Promoted quote requires bounded policy scope' USING ERRCODE='23514'; END IF;
  IF EXISTS(SELECT 1 FROM jsonb_array_elements(draft.policy->'promotion'->'countries') c(value)
   WHERE jsonb_typeof(c.value) IS DISTINCT FROM 'string')
   OR EXISTS(SELECT 1 FROM jsonb_array_elements(draft.policy->'promotion'->'products') p(value)
    WHERE jsonb_typeof(p.value) IS DISTINCT FROM 'object'
     OR p.value - ARRAY['sku','publication_revision'] IS DISTINCT FROM '{}'::jsonb
     OR NOT p.value ?& ARRAY['sku','publication_revision']
     OR jsonb_typeof(p.value->'sku') IS DISTINCT FROM 'string'
     OR jsonb_typeof(p.value->'publication_revision') IS DISTINCT FROM 'number')
   OR (CASE draft.policy->'promotion'->'discount'->>'kind'
    WHEN 'fixed' THEN (draft.policy->'promotion'->'discount') - ARRAY['kind','minor_units'] IS DISTINCT FROM '{}'::jsonb
     OR NOT draft.policy->'promotion'->'discount' ? 'minor_units'
     OR jsonb_typeof(draft.policy->'promotion'->'discount'->'minor_units') IS DISTINCT FROM 'number'
    WHEN 'percentage' THEN (draft.policy->'promotion'->'discount') - ARRAY['kind','basis_points'] IS DISTINCT FROM '{}'::jsonb
     OR NOT draft.policy->'promotion'->'discount' ? 'basis_points'
     OR jsonb_typeof(draft.policy->'promotion'->'discount'->'basis_points') IS DISTINCT FROM 'number'
    ELSE true END) THEN
   RAISE EXCEPTION 'Promoted quote requires closed policy terms' USING ERRCODE='23514'; END IF;
  IF (draft.policy->'promotion'->>'valid_from')::numeric > NEW.created_at
   OR (draft.policy->'promotion'->>'expires_at')::numeric < NEW.expires_at
   OR (draft.policy->'promotion'->>'expires_at')::numeric > 9007199254740991 THEN
   RAISE EXCEPTION 'Promoted quote exceeds policy window' USING ERRCODE='23514'; END IF;
  promoted=NEW.snapshot;
  IF jsonb_typeof(promoted) IS DISTINCT FROM 'object'
   OR promoted - ARRAY['schema_version','original','promotion','allocation_version','discount_minor','lines','shipping','total'] IS DISTINCT FROM '{}'::jsonb
   OR NOT promoted ?& ARRAY['schema_version','original','promotion','allocation_version','discount_minor','lines','shipping','total']
   OR promoted->'schema_version' IS DISTINCT FROM '2'::jsonb
   OR promoted->'promotion' IS DISTINCT FROM jsonb_build_object('id',NEW.promotion_id,'revision',NEW.promotion_revision,'review_revision',NEW.promotion_review_revision)
   OR promoted->'allocation_version' IS DISTINCT FROM '1'::jsonb
   OR jsonb_typeof(promoted->'discount_minor') IS DISTINCT FROM 'number'
   OR coalesce(promoted->>'discount_minor','') !~ '^[1-9][0-9]{0,15}$'
   OR jsonb_typeof(promoted->'lines') IS DISTINCT FROM 'array'
   OR NOT promoted_quote_amount_shape(promoted->'shipping')
   OR NOT promoted_quote_amount_shape(promoted->'total') THEN
   RAISE EXCEPTION 'Promoted quote requires a bounded closed envelope' USING ERRCODE='23514'; END IF;
  IF (promoted->>'discount_minor')::numeric > 9007199254740991 THEN
   RAISE EXCEPTION 'Promoted quote discount exceeds bound' USING ERRCODE='23514'; END IF;
  source=promoted->'original';
  IF jsonb_typeof(source) IS DISTINCT FROM 'object'
   OR source - ARRAY['schema_version','binding','currency','lines','shipping','shipping_vat_basis_points','total'] IS DISTINCT FROM '{}'::jsonb
   OR NOT source ?& ARRAY['schema_version','binding','currency','lines','shipping','shipping_vat_basis_points','total']
   OR promoted->'shipping' IS DISTINCT FROM source->'shipping'
   OR jsonb_typeof(source->'lines') IS DISTINCT FROM 'array'
   OR NOT promoted_quote_amount_shape(source->'shipping')
   OR NOT promoted_quote_amount_shape(source->'total')
   OR jsonb_typeof(source->'shipping_vat_basis_points') IS DISTINCT FROM 'number' THEN
   RAISE EXCEPTION 'Promoted quote requires its exact original envelope' USING ERRCODE='23514'; END IF;
  IF jsonb_array_length(promoted->'lines') IS DISTINCT FROM jsonb_array_length(source->'lines')
   OR EXISTS(SELECT 1 FROM jsonb_array_elements(promoted->'lines') WITH ORDINALITY x(value,n)
     LEFT JOIN LATERAL (SELECT source->'lines'->((x.n-1)::integer) AS value) s ON true
     WHERE jsonb_typeof(s.value) IS DISTINCT FROM 'object'
      OR s.value - ARRAY['sku','publication_revision','bom_revision','quantity','unit_gross_minor','vat_basis_points','amounts'] IS DISTINCT FROM '{}'::jsonb
      OR NOT s.value ?& ARRAY['sku','publication_revision','bom_revision','quantity','unit_gross_minor','vat_basis_points','amounts']
      OR NOT promoted_quote_amount_shape(s.value->'amounts')
      OR jsonb_typeof(x.value) IS DISTINCT FROM 'object'
      OR x.value - ARRAY['sku','discount_minor','amounts'] IS DISTINCT FROM '{}'::jsonb
      OR NOT x.value ?& ARRAY['sku','discount_minor','amounts']
      OR jsonb_typeof(x.value->'sku') IS DISTINCT FROM 'string'
      OR x.value->'sku' IS DISTINCT FROM s.value->'sku'
      OR jsonb_typeof(x.value->'discount_minor') IS DISTINCT FROM 'number'
      OR coalesce(x.value->>'discount_minor','') !~ '^(0|[1-9][0-9]{0,15})$'
      OR NOT promoted_quote_amount_shape(x.value->'amounts')) THEN
   RAISE EXCEPTION 'Promoted quote requires bounded original SKU lines' USING ERRCODE='23514'; END IF;
  IF EXISTS(SELECT 1 FROM jsonb_array_elements(promoted->'lines') x(value)
    WHERE (x.value->>'discount_minor')::numeric>9007199254740991) THEN
   RAISE EXCEPTION 'Promoted quote discount exceeds bound' USING ERRCODE='23514'; END IF;
  IF EXISTS(SELECT 1 FROM jsonb_array_elements(promoted->'lines') WITH ORDINALITY x(value,n)
    WHERE (x.value->>'discount_minor')::numeric>0 AND NOT EXISTS(
     SELECT 1 FROM jsonb_array_elements(draft.policy->'promotion'->'products') p(value)
     WHERE p.value->'sku'=x.value->'sku'
      AND p.value->'publication_revision'=source->'lines'->((x.n-1)::integer)->'publication_revision')) THEN
   RAISE EXCEPTION 'Promoted quote discount requires exact product scope' USING ERRCODE='23514'; END IF;
  NEW.promotion_transaction_id=pg_current_xact_id();
 ELSE RAISE EXCEPTION 'Promotion identity must be complete' USING ERRCODE='23514';
 END IF;
 binding:=jsonb_build_object('quote_id',NEW.id,'basket_id',NEW.basket_id,'basket_revision',NEW.basket_revision,'locale',NEW.locale,
  'country',NEW.country,'postcode',NEW.postcode,'policy_id',NEW.policy_id,'policy_revision',NEW.policy_revision,
  'policy_review_revision',NEW.policy_review_revision,'created_at',NEW.created_at,'expires_at',NEW.expires_at);
 IF source->'binding' IS DISTINCT FROM binding OR source->'schema_version' IS DISTINCT FROM '1'::jsonb
  OR source->'currency' IS DISTINCT FROM '"EUR"'::jsonb OR jsonb_typeof(source->'lines') IS DISTINCT FROM 'array' THEN
  RAISE EXCEPTION 'Quote requires its exact closed binding' USING ERRCODE='23514'; END IF;
 IF jsonb_array_length(source->'lines') NOT BETWEEN 1 AND 20
  OR NOT EXISTS(SELECT 1 FROM market_policy_reviews r JOIN market_policy_anchors a USING(id)
   WHERE r.id=NEW.policy_id AND r.draft_revision=NEW.policy_revision AND r.revision=NEW.policy_review_revision
    AND r.decision='approved' AND a.country=NEW.country AND a.postcode=NEW.postcode) THEN
  RAISE EXCEPTION 'Quote requires approved exact-destination source' USING ERRCODE='23514'; END IF;
 SELECT jsonb_agg(jsonb_build_object('sku',l->>'sku','quantity',l->'quantity') ORDER BY (l->>'sku') COLLATE "C") INTO intent
  FROM jsonb_array_elements(source->'lines') l;
 SELECT lines INTO original FROM guest_basket_effects WHERE basket_id=NEW.basket_id AND revision=NEW.basket_revision;
 IF intent IS DISTINCT FROM original THEN RAISE EXCEPTION 'Quote lines require exact immutable basket intent' USING ERRCODE='23514'; END IF;
 FOR item IN SELECT value FROM jsonb_array_elements(source->'lines') LOOP
  IF NOT EXISTS(SELECT 1 FROM catalogue_publication_revisions p WHERE p.sku=item->>'sku' AND p.locale=NEW.locale
   AND p.revision=(item->>'publication_revision')::integer AND p.choice->>'action'='activate'
   AND (p.choice->'selection'->'facts'->>'bom_revision')::integer=(item->>'bom_revision')::integer) THEN
   RAISE EXCEPTION 'Quote requires exact published BOM source' USING ERRCODE='23514'; END IF;
 END LOOP;
 RETURN NEW;
END;
$$;

-- Prevent a transaction from inserting a quote and then superseding the source
-- under its own held anchor lock. Competing transactions serialize on that lock.
CREATE FUNCTION guard_promotion_quote_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM guest_quote_snapshots q WHERE q.promotion_id=NEW.id
  AND q.promotion_transaction_id=pg_current_xact_id()) THEN
  RAISE EXCEPTION 'Promoted quote source is sealed for this transaction' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_promotion_quote_seal BEFORE INSERT ON promotion_policy_drafts FOR EACH ROW EXECUTE FUNCTION guard_promotion_quote_seal();
CREATE TRIGGER aa_promotion_quote_seal BEFORE INSERT ON promotion_policy_reviews FOR EACH ROW EXECUTE FUNCTION guard_promotion_quote_seal();

-- No legacy order path may consume a promoted indicative snapshot.
CREATE FUNCTION guard_checkout_unpromoted_quote() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM guest_quote_snapshots q WHERE q.id=NEW.quote_id
  AND (q.promotion_id IS NOT NULL OR q.promotion_revision IS NOT NULL
   OR q.promotion_review_revision IS NOT NULL OR q.snapshot->'schema_version'='2'::jsonb)) THEN
  RAISE EXCEPTION 'Promoted indicative quotes cannot create orders' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_checkout_unpromoted_quote BEFORE INSERT ON checkout_orders FOR EACH ROW EXECUTE FUNCTION guard_checkout_unpromoted_quote();
