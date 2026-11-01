-- No stock is seeded; only complete active original reservations can settle.
CREATE TABLE order_stock_settlements (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 fence_key integer GENERATED ALWAYS AS IDENTITY UNIQUE CHECK(fence_key>0),
 order_id text UNIQUE NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 group_id text UNIQUE NOT NULL REFERENCES basket_hold_groups(id),
 actor text NOT NULL CHECK(actor='payment-settlement'),
 settled_at bigint NOT NULL CHECK(settled_at BETWEEN 0 AND 9007199254740991),
 footprint jsonb NOT NULL CHECK(jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=1048576)
);
CREATE FUNCTION order_stock_digest(s order_stock_settlements) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',s.id,'order_id',s.order_id,
   'attempt_id',s.attempt_id,'recognition_id',s.recognition_id,'group_id',s.group_id,
   'actor',s.actor,'settled_at',s.settled_at,'footprint',s.footprint,'fence_key',s.fence_key)::text,'UTF8')),'hex')
$$;

-- Bounded original membership, sealed BOM and complete reserve/terminal custody.
-- This helper grants no current financial or physical authority by itself.
CREATE FUNCTION order_stock_source(wanted_order text) RETURNS jsonb LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; g basket_hold_groups%ROWTYPE; h stock_holds%ROWTYPE;
 m basket_hold_members%ROWTYPE; line stock_hold_lines%ROWTYPE; movement stock_movements%ROWTYPE;
 members jsonb := '[]'; components jsonb; n bigint := 0; lines bigint; effects bigint; terminal text;
 expected bigint; total bigint := 0; reserve_sequence bigint;
BEGIN
 SELECT * INTO owned FROM checkout_orders WHERE id=wanted_order;
 SELECT * INTO g FROM basket_hold_groups WHERE id=owned.stock_group_id;
 IF owned.id IS NULL OR g.id IS NULL OR NOT g.sealed OR g.owner_id<>owned.id
  OR g.created_at<>owned.created_at OR g.expires_at<>owned.hold_expires_at
  OR g.members_count NOT BETWEEN 1 AND 20
  OR (SELECT count(*) FROM (SELECT 1 FROM basket_hold_members WHERE group_id=g.id LIMIT 21) bounded)<>g.members_count
  OR (SELECT count(*) FROM (SELECT 1 FROM stock_holds WHERE basket_group_id=g.id LIMIT 21) bounded)<>g.members_count
  OR jsonb_typeof(owned.snapshot->'quote'->'lines') IS DISTINCT FROM 'array'
  OR jsonb_array_length(owned.snapshot->'quote'->'lines')<>g.members_count THEN
  RAISE EXCEPTION 'Settlement requires complete original group' USING ERRCODE='23514'; END IF;
 FOR m IN SELECT * FROM basket_hold_members WHERE group_id=g.id ORDER BY hold_id LIMIT 21 LOOP
  n=n+1; SELECT * INTO h FROM stock_holds WHERE id=m.hold_id;
  IF n>20 OR h.id IS NULL OR h.id !~ '^[a-f0-9]{32}$' OR h.basket_group_id IS DISTINCT FROM g.id
   OR (h.owner_id,h.sku,h.revision,h.units,h.created_at,h.expires_at)
     IS DISTINCT FROM (g.owner_id,m.sku,m.revision,m.units,g.created_at,g.expires_at)
   OR h.units NOT BETWEEN 1 AND 1000 OR h.revision<=0
   OR h.state NOT IN ('active','released','expired','consumed')
   OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(owned.snapshot->'quote'->'lines') q
     WHERE q->>'sku'=m.sku AND q->'bom_revision'=to_jsonb(m.revision) AND q->'quantity'=to_jsonb(m.units))
   OR NOT EXISTS(SELECT 1 FROM catalogue_boms WHERE sku=m.sku AND revision=m.revision AND sealed) THEN
   RAISE EXCEPTION 'Settlement member is not exact original selection' USING ERRCODE='23514'; END IF;
  SELECT count(*) INTO lines FROM (SELECT 1 FROM catalogue_bom_lines WHERE sku=m.sku AND revision=m.revision LIMIT 65) bounded;
  IF lines NOT BETWEEN 1 AND 64 OR (SELECT count(*) FROM (SELECT 1 FROM stock_hold_lines WHERE hold_id=h.id LIMIT 65) bounded)<>lines THEN
   RAISE EXCEPTION 'Settlement requires bounded complete BOM' USING ERRCODE='23514'; END IF;
  components='[]'; effects=0;
  terminal=CASE h.state WHEN 'released' THEN 'release-hold' WHEN 'expired' THEN 'expire-hold' WHEN 'consumed' THEN 'consume-hold' ELSE NULL END;
  FOR line IN SELECT * FROM stock_hold_lines WHERE hold_id=h.id ORDER BY component_code LIMIT 65 LOOP
   total=total+1;
   SELECT b.quantity::bigint*h.units INTO expected FROM catalogue_bom_lines b
     JOIN physical_components p ON p.code=b.component_code
     WHERE b.sku=h.sku AND b.revision=h.revision AND b.component_code=line.component_code
       AND b.quantity BETWEEN 1 AND 10000 AND p.kind IN ('cover','foam');
   IF total>1280 OR expected IS NULL OR line.quantity IS DISTINCT FROM expected THEN
    RAISE EXCEPTION 'Settlement requires exact original component allocation' USING ERRCODE='23514'; END IF;
   SELECT * INTO movement FROM stock_movements WHERE hold_id=h.id AND component_code=line.component_code AND action='reserve';
   IF movement.sequence IS NULL OR movement.quantity<>line.quantity OR movement.actor<>owned.id
    OR movement.reason<>'sealed BOM hold transition'
    OR movement.effect_key<>('hold:'||h.id||':reserve:'||line.component_code)
    OR (movement.on_hand_delta,movement.quarantine_delta,movement.reserved_delta) IS DISTINCT FROM (0::bigint,0::bigint,line.quantity)
    OR movement.on_hand_after<0 OR movement.quarantine_after<0 OR movement.reserved_after<line.quantity
    OR movement.quarantine_after::numeric+movement.reserved_after>movement.on_hand_after THEN
    RAISE EXCEPTION 'Settlement requires exact original reserve movement' USING ERRCODE='23514'; END IF;
   reserve_sequence=movement.sequence; effects=effects+1;
   IF terminal IS NOT NULL THEN
    SELECT * INTO movement FROM stock_movements WHERE hold_id=h.id AND component_code=line.component_code AND action=terminal;
    IF movement.sequence IS NULL OR movement.sequence<=reserve_sequence OR movement.quantity<>line.quantity
     OR movement.effect_key<>('hold:'||h.id||':'||terminal||':'||line.component_code)
     OR movement.reason<>'sealed BOM hold transition' OR btrim(movement.actor)='' OR length(movement.actor)>128
     OR movement.on_hand_delta IS DISTINCT FROM (CASE WHEN terminal='consume-hold' THEN -line.quantity ELSE 0 END)
     OR movement.quarantine_delta<>0 OR movement.reserved_delta<>-line.quantity
     OR movement.on_hand_after<0 OR movement.quarantine_after<0 OR movement.reserved_after<0
     OR movement.quarantine_after::numeric+movement.reserved_after>movement.on_hand_after THEN
     RAISE EXCEPTION 'Terminal original hold has invalid custody' USING ERRCODE='23514'; END IF;
    effects=effects+1;
   END IF;
   components=components||jsonb_build_array(jsonb_build_object('component_code',line.component_code,'quantity',line.quantity));
  END LOOP;
  IF (SELECT count(*) FROM (SELECT 1 FROM stock_movements WHERE hold_id=h.id LIMIT 129) bounded)<>effects THEN
   RAISE EXCEPTION 'Hold has missing or extra effects' USING ERRCODE='23514'; END IF;
  members=members||jsonb_build_array(jsonb_build_object('hold_id',h.id,'sku',h.sku,'revision',h.revision,'units',h.units,'components',components));
 END LOOP;
 RETURN members;
END;
$$;

CREATE FUNCTION order_stock_received(s order_stock_settlements) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE recognition payment_recognitions%ROWTYPE; owned checkout_orders%ROWTYPE;
BEGIN
 SELECT * INTO recognition FROM payment_recognitions WHERE id=s.recognition_id;
 SELECT * INTO owned FROM checkout_orders WHERE id=s.order_id;
 IF recognition.id IS NULL OR owned.id IS NULL OR recognition.order_id<>owned.id
  OR recognition.attempt_id<>s.attempt_id OR recognition.state<>'received'
  OR s.group_id IS DISTINCT FROM owned.stock_group_id OR s.actor<>'payment-settlement'
  OR s.settled_at<recognition.recorded_at OR s.settled_at<owned.created_at OR s.settled_at>=owned.hold_expires_at
  OR NOT payment_recognition_source(recognition,true)
  OR NOT EXISTS(SELECT 1 FROM payment_recognition_current p WHERE p.order_id=owned.id AND p.recognition_id=recognition.id)
  OR EXISTS(SELECT 1 FROM payment_recognitions WHERE order_id=owned.id AND through_observation>recognition.through_observation)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id AND e.kind='payment.recognized.v1'
    AND e.created_at=recognition.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,
      'attempt_id',recognition.attempt_id,'recognition_id',recognition.id,'recognition_sha256',payment_recognition_digest(recognition))) THEN
  RETURN false; END IF;
 RETURN true;
END;
$$;

CREATE FUNCTION guard_order_stock_settlement() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE members jsonb; components jsonb := '[]'; demand record; balance stock_balances%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Settlement is immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN
  RAISE EXCEPTION 'Settlement requires read committed' USING ERRCODE='23514'; END IF;
 -- Keep lock and evidence inspection separate, with fresh VOLATILE RC queries.
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF NOT order_stock_received(NEW) THEN
  RAISE EXCEPTION 'Settlement requires current exact received recognition' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.group_id ORDER BY id LIMIT 21 FOR UPDATE;
 members=order_stock_source(NEW.order_id);
 PERFORM 1 FROM stock_balances WHERE component_code IN (
   SELECT line->>'component_code' FROM jsonb_array_elements(members) m
     CROSS JOIN LATERAL jsonb_array_elements(m->'components') line)
   ORDER BY component_code LIMIT 1281 FOR UPDATE;
 IF EXISTS(SELECT 1 FROM stock_holds WHERE basket_group_id=NEW.group_id AND state<>'active') THEN
  RAISE EXCEPTION 'New settlement cannot adopt terminal holds' USING ERRCODE='23514'; END IF;
 FOR demand IN SELECT line->>'component_code' AS code,sum((line->>'quantity')::bigint)::bigint AS quantity
   FROM jsonb_array_elements(members) m CROSS JOIN LATERAL jsonb_array_elements(m->'components') line
   GROUP BY line->>'component_code' ORDER BY line->>'component_code' LOOP
  SELECT * INTO balance FROM stock_balances WHERE component_code=demand.code;
  IF balance.component_code IS NULL OR balance.on_hand<0 OR balance.quarantine<0 OR balance.reserved<demand.quantity
   OR balance.quarantine::numeric+balance.reserved>balance.on_hand THEN
   RAISE EXCEPTION 'Settlement requires actual backed locked stock' USING ERRCODE='23514'; END IF;
  components=components||jsonb_build_array(jsonb_build_object('component_code',demand.code,'quantity',demand.quantity,
    'before',jsonb_build_object('on_hand',balance.on_hand,'quarantine',balance.quarantine,'reserved',balance.reserved),
    'after',jsonb_build_object('on_hand',balance.on_hand-demand.quantity,'quarantine',balance.quarantine,'reserved',balance.reserved-demand.quantity)));
 END LOOP;
 IF NEW.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'members',members,'components',components) THEN
  RAISE EXCEPTION 'Settlement footprint must capture actual original allocation and physical before' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_order_stock_settlement BEFORE INSERT OR UPDATE OR DELETE ON order_stock_settlements FOR EACH ROW EXECUTE FUNCTION guard_order_stock_settlement();

-- Exact immutable effects, including member-order intermediate balances. Historical
-- reads deliberately pass false: legitimate later warehouse effects are independent.
CREATE FUNCTION order_stock_effects_exact(s order_stock_settlements, current_balances boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE members jsonb; m jsonb; line jsonb; c jsonb; balances jsonb := '{}';
 expected jsonb; movement stock_movements%ROWTYPE; previous bigint := 0;
 remaining_on bigint; remaining_reserved bigint;
BEGIN
 members=order_stock_source(s.order_id);
 IF s.footprint->'members' IS DISTINCT FROM members
  OR EXISTS(SELECT 1 FROM stock_holds WHERE basket_group_id=s.group_id AND state<>'consumed') THEN RETURN false; END IF;
 FOR c IN SELECT value FROM jsonb_array_elements(s.footprint->'components') LOOP
  balances=jsonb_set(balances,ARRAY[c->>'component_code'],c->'before');
 END LOOP;
 FOR m IN SELECT value FROM jsonb_array_elements(members) LOOP
  FOR line IN SELECT value FROM jsonb_array_elements(m->'components') LOOP
   SELECT * INTO movement FROM stock_movements WHERE hold_id=m->>'hold_id'
     AND component_code=line->>'component_code' AND action='consume-hold';
   expected=balances->(line->>'component_code');
   remaining_on=(expected->>'on_hand')::bigint-(line->>'quantity')::bigint;
   remaining_reserved=(expected->>'reserved')::bigint-(line->>'quantity')::bigint;
   IF movement.sequence IS NULL OR movement.sequence<=previous OR movement.actor<>'payment-settlement'
    OR movement.on_hand_after IS DISTINCT FROM remaining_on
    OR movement.quarantine_after IS DISTINCT FROM (expected->>'quarantine')::bigint
    OR movement.reserved_after IS DISTINCT FROM remaining_reserved THEN RETURN false; END IF;
   balances=jsonb_set(balances,ARRAY[line->>'component_code'],jsonb_build_object('on_hand',remaining_on,
     'quarantine',movement.quarantine_after,'reserved',remaining_reserved));
   previous=movement.sequence;
  END LOOP;
 END LOOP;
 FOR c IN SELECT value FROM jsonb_array_elements(s.footprint->'components') LOOP
  IF c->'after' IS DISTINCT FROM balances->(c->>'component_code') THEN RETURN false; END IF;
  IF current_balances AND NOT EXISTS(SELECT 1 FROM stock_balances b WHERE b.component_code=c->>'component_code'
    AND c->'after'=jsonb_build_object('on_hand',b.on_hand,'quarantine',b.quarantine,'reserved',b.reserved)) THEN RETURN false; END IF;
 END LOOP;
 RETURN true;
END;
$$;
CREATE FUNCTION require_complete_order_stock_settlement() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NOT order_stock_received(NEW) OR NOT order_stock_effects_exact(NEW,true)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=NEW.order_id AND e.kind='order.stock_settled.v1'
    AND e.created_at=NEW.settled_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',NEW.order_id,
      'settlement_id',NEW.id,'settlement_sha256',order_stock_digest(NEW))) THEN
  RAISE EXCEPTION 'Settlement requires complete exact physical effects and event' USING ERRCODE='23514'; END IF;
 -- Always run the FULL proof first. A caller can already own an advisory lock,
 -- which may deny its own later mutations but can never establish completion.
 PERFORM pg_advisory_xact_lock(1937011563,NEW.fence_key);
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_order_stock_settlement AFTER INSERT ON order_stock_settlements DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_order_stock_settlement();

ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1'));

CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE; settlement order_stock_settlements%ROWTYPE;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Commerce events are immutable' USING ERRCODE='23514'; END IF;
 IF NEW.kind='checkout.order_created.v1' THEN
  IF NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',NEW.aggregate_id)
   OR NOT EXISTS(SELECT 1 FROM checkout_orders o WHERE o.id=NEW.aggregate_id AND o.created_at=NEW.created_at) THEN
   RAISE EXCEPTION 'Order event requires exact original binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.attempt_prepared.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_attempts a WHERE a.order_id=NEW.aggregate_id AND a.created_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id)) THEN
   RAISE EXCEPTION 'Prepared payment event requires exact attempt binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.reconciliation_required.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_dispatch_starts s JOIN payment_attempts a ON a.id=s.attempt_id WHERE a.order_id=NEW.aggregate_id AND s.started_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id)) THEN
   RAISE EXCEPTION 'Payment recovery event requires exact dispatch binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.observation_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_observations o JOIN payment_attempts a ON a.id=o.attempt_id WHERE a.order_id=NEW.aggregate_id AND o.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'observation_id',o.id)) THEN
   RAISE EXCEPTION 'Observation event requires exact historical fact binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.search_page_recorded.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_search_pages p JOIN payment_attempts a ON a.id=p.attempt_id WHERE a.order_id=NEW.aggregate_id AND p.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'page_id',p.id)) THEN
   RAISE EXCEPTION 'Search page event requires exact discovery binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.identity_resolved.v1' THEN
  IF NOT EXISTS(SELECT 1 FROM payment_identity_resolutions r JOIN payment_attempts a ON a.id=r.attempt_id WHERE a.order_id=NEW.aggregate_id AND r.recorded_at=NEW.created_at AND NEW.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'resolution_id',r.id)) THEN
   RAISE EXCEPTION 'Resolution event requires exact evidence binding' USING ERRCODE='23514';
  END IF;
 ELSIF NEW.kind='payment.recognized.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Recognition event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO recognition FROM payment_recognitions WHERE id=NEW.payload->>'recognition_id';
  PERFORM 1 FROM payment_attempts WHERE id=recognition.attempt_id FOR UPDATE;
  IF recognition.id IS NULL OR NOT payment_recognition_source(recognition,false)
   OR NEW.aggregate_id IS DISTINCT FROM recognition.order_id OR NEW.created_at IS DISTINCT FROM recognition.recorded_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',recognition.order_id,
     'attempt_id',recognition.attempt_id,'recognition_id',recognition.id,'recognition_sha256',payment_recognition_digest(recognition)) THEN
   RAISE EXCEPTION 'Recognition event requires exact immutable journal binding' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_settled.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Settlement event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO settlement FROM order_stock_settlements WHERE id=NEW.payload->>'settlement_id';
  PERFORM 1 FROM payment_attempts WHERE id=settlement.attempt_id FOR UPDATE;
  IF settlement.id IS NULL OR NEW.aggregate_id IS DISTINCT FROM settlement.order_id
   OR NEW.created_at IS DISTINCT FROM settlement.settled_at
   OR NOT order_stock_effects_exact(settlement,false)
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',settlement.order_id,
     'settlement_id',settlement.id,'settlement_sha256',order_stock_digest(settlement)) THEN
   RAISE EXCEPTION 'Settlement event requires exact immutable receipt and effects' USING ERRCODE='23514'; END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;

-- The two-int advisory namespace is isolated from existing bigint advisory locks
-- (objsubid=1). Only THIS backend's locks matter; completed other transactions
-- do not freeze future legitimate warehouse or financial activity.
CREATE FUNCTION guard_order_stock_completion_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE seal record; settlement order_stock_settlements%ROWTYPE;
 attempts text[] := '{}'; orders text[] := '{}'; holds text[] := '{}';
 components text[] := '{}'; groups text[] := '{}';
BEGIN
 IF TG_TABLE_NAME IN ('payment_observations','payment_identity_resolutions','payment_recognitions') THEN
  IF TG_OP<>'DELETE' THEN attempts=array_append(attempts,NEW.attempt_id); END IF;
  IF TG_OP<>'INSERT' THEN attempts=array_append(attempts,OLD.attempt_id); END IF;
 ELSIF TG_TABLE_NAME='payment_recognition_current' THEN
  IF TG_OP<>'DELETE' THEN orders=array_append(orders,NEW.order_id); END IF;
  IF TG_OP<>'INSERT' THEN orders=array_append(orders,OLD.order_id); END IF;
 ELSIF TG_TABLE_NAME='stock_balances' THEN
  IF TG_OP<>'DELETE' THEN components=array_append(components,NEW.component_code); END IF;
  IF TG_OP<>'INSERT' THEN components=array_append(components,OLD.component_code); END IF;
 ELSIF TG_TABLE_NAME='stock_holds' THEN
  IF TG_OP<>'DELETE' THEN holds=array_append(holds,NEW.id); groups=array_append(groups,NEW.basket_group_id); END IF;
  IF TG_OP<>'INSERT' THEN holds=array_append(holds,OLD.id); groups=array_append(groups,OLD.basket_group_id); END IF;
 ELSIF TG_TABLE_NAME='stock_movements' THEN
  IF TG_OP<>'DELETE' THEN holds=array_append(holds,NEW.hold_id); components=array_append(components,NEW.component_code); END IF;
  IF TG_OP<>'INSERT' THEN holds=array_append(holds,OLD.hold_id); components=array_append(components,OLD.component_code); END IF;
 END IF;
 -- Drive from the bounded live backend lock set, then use the UNIQUE fence key.
 -- Never scan all historical receipts on every warehouse mutation.
 FOR seal IN SELECT objid::bigint AS key FROM pg_locks
   WHERE locktype='advisory' AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
     AND pid=pg_backend_pid() AND mode='ExclusiveLock' AND granted
     AND classid=1937011563::oid AND objsubid=2 LOOP
  SELECT * INTO settlement FROM order_stock_settlements WHERE fence_key=seal.key;
  IF settlement.id IS NULL THEN CONTINUE; END IF;
  IF octet_length(settlement.footprint::text)>1048576 THEN
   RAISE EXCEPTION 'Sealed settlement has invalid bounded custody' USING ERRCODE='23514'; END IF;
  IF settlement.attempt_id=ANY(attempts) OR settlement.order_id=ANY(orders) OR settlement.group_id=ANY(groups)
   OR EXISTS(SELECT 1 FROM jsonb_array_elements(settlement.footprint->'members') member WHERE member->>'hold_id'=ANY(holds))
   OR EXISTS(SELECT 1 FROM jsonb_array_elements(settlement.footprint->'components') component WHERE component->>'component_code'=ANY(components)) THEN
   RAISE EXCEPTION 'Completed settlement source cannot change again in this transaction' USING ERRCODE='23514'; END IF;
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_observations FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_identity_resolutions FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognitions FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON payment_recognition_current FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON stock_balances FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON stock_holds FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON stock_movements FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
