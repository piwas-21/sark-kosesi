-- Empty private order history. No seller, order, charge, fulfilment or dispatch is installed.
CREATE TABLE checkout_orders (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 basket_id text NOT NULL REFERENCES guest_baskets(id),
 basket_revision integer NOT NULL CHECK(basket_revision>0),
 quote_id text UNIQUE NOT NULL REFERENCES guest_quote_snapshots(id),
 effect_key text NOT NULL CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'),
 stock_group_id text UNIQUE NOT NULL REFERENCES basket_hold_groups(id),
 created_at bigint NOT NULL CHECK(created_at>=0),
 hold_expires_at bigint NOT NULL CHECK(hold_expires_at>created_at AND hold_expires_at-created_at<=1800 AND hold_expires_at<=9007199254740991),
 intent jsonb NOT NULL CHECK(jsonb_typeof(intent)='object' AND octet_length(intent::text)<=16384),
 snapshot jsonb NOT NULL CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=65536),
 UNIQUE(basket_id,effect_key)
);
CREATE FUNCTION guard_checkout_order() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE quote guest_quote_snapshots%ROWTYPE; original_market jsonb; members jsonb; selected jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Order snapshots are immutable' USING ERRCODE='23514'; END IF;
 SELECT * INTO quote FROM guest_quote_snapshots WHERE id=NEW.quote_id AND basket_id=NEW.basket_id AND basket_revision=NEW.basket_revision;
 SELECT policy INTO original_market FROM market_policy_drafts WHERE id=quote.policy_id AND revision=quote.policy_revision;
 IF quote.id IS NULL OR original_market IS NULL
  OR NEW.snapshot->'schema_version' IS DISTINCT FROM '1'::jsonb
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
 IF NOT EXISTS(SELECT 1 FROM basket_hold_groups g WHERE g.id=NEW.stock_group_id AND g.owner_id=NEW.id AND g.created_at=NEW.created_at AND g.expires_at=NEW.hold_expires_at AND g.sealed AND g.members_count=jsonb_array_length(quote.snapshot->'lines')) THEN
  RAISE EXCEPTION 'Order requires complete owned initial reservation' USING ERRCODE='23514';
 END IF;
 SELECT jsonb_agg(jsonb_build_object('sku',m.sku,'bom_revision',m.revision,'quantity',m.units) ORDER BY m.sku COLLATE "C") INTO members FROM basket_hold_members m WHERE m.group_id=NEW.stock_group_id;
 SELECT jsonb_agg(jsonb_build_object('sku',l->>'sku','bom_revision',l->'bom_revision','quantity',l->'quantity') ORDER BY (l->>'sku') COLLATE "C") INTO selected FROM jsonb_array_elements(quote.snapshot->'lines') l;
 IF members IS DISTINCT FROM selected THEN RAISE EXCEPTION 'Order reservations require exact quote membership' USING ERRCODE='23514'; END IF;
 SELECT jsonb_agg(jsonb_build_object('sku',p->>'sku','bom_revision',p->'bom_revision','publication_revision',p->'publication_revision') ORDER BY (p->>'sku') COLLATE "C") INTO members FROM jsonb_array_elements(NEW.snapshot->'products') p;
 SELECT jsonb_agg(jsonb_build_object('sku',l->>'sku','bom_revision',l->'bom_revision','publication_revision',l->'publication_revision') ORDER BY (l->>'sku') COLLATE "C") INTO selected FROM jsonb_array_elements(quote.snapshot->'lines') l;
 IF members IS DISTINCT FROM selected THEN RAISE EXCEPTION 'Order merchandise requires exact immutable sources' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_checkout_order BEFORE INSERT OR UPDATE OR DELETE ON checkout_orders FOR EACH ROW EXECUTE FUNCTION guard_checkout_order();
CREATE TABLE commerce_outbox (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 aggregate_id text NOT NULL REFERENCES checkout_orders(id),
 kind text NOT NULL CHECK(kind='checkout.order_created.v1'),
 payload jsonb NOT NULL CHECK(jsonb_typeof(payload)='object' AND octet_length(payload::text)<=4096),
 created_at bigint NOT NULL CHECK(created_at>=0),
 UNIQUE(aggregate_id,kind)
);
CREATE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP<>'INSERT' OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',NEW.aggregate_id)
  OR NOT EXISTS(SELECT 1 FROM checkout_orders o WHERE o.id=NEW.aggregate_id AND o.created_at=NEW.created_at) THEN
  RAISE EXCEPTION 'Commerce event is immutable and bound to its order' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_commerce_outbox BEFORE INSERT OR UPDATE OR DELETE ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION guard_commerce_outbox();
CREATE FUNCTION require_complete_checkout() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM commerce_outbox WHERE aggregate_id=NEW.id AND kind='checkout.order_created.v1')
  OR EXISTS(SELECT 1 FROM stock_holds WHERE basket_group_id=NEW.stock_group_id AND state<>'active') THEN
  RAISE EXCEPTION 'Initial checkout requires outbox and active whole-basket holds' USING ERRCODE='23514';
 END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_checkout AFTER INSERT ON checkout_orders DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_checkout();
