-- One real order has one physical allocation across original and late paths.
CREATE TABLE order_stock_allocations (
 order_id text PRIMARY KEY REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 kind text NOT NULL CHECK(kind IN ('original','late')),
 receipt_id text NOT NULL CHECK(receipt_id ~ '^[a-f0-9]{32}$'),
 UNIQUE(kind,receipt_id)
);
CREATE TABLE late_stock_allocations (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 fence_key integer GENERATED ALWAYS AS IDENTITY UNIQUE CHECK(fence_key>0),
 order_id text UNIQUE NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 original_group_id text UNIQUE NOT NULL REFERENCES basket_hold_groups(id),
 fresh_group_id text UNIQUE NOT NULL REFERENCES basket_hold_groups(id),
 actor text NOT NULL CHECK(actor='late-payment-allocation'),
 allocated_at bigint NOT NULL CHECK(allocated_at BETWEEN 0 AND 9007199254740991),
 expires_at bigint NOT NULL CHECK(expires_at BETWEEN 1 AND 9007199254740991),
 footprint jsonb NOT NULL CHECK(jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=2097152),
 CHECK(original_group_id<>fresh_group_id AND allocated_at>=expires_at-60 AND allocated_at<expires_at)
);
CREATE TABLE late_stock_review_cases (
 id text PRIMARY KEY CHECK(id ~ '^[a-f0-9]{32}$'),
 fence_key integer GENERATED ALWAYS AS IDENTITY UNIQUE CHECK(fence_key>0),
 order_id text UNIQUE NOT NULL REFERENCES checkout_orders(id),
 attempt_id text NOT NULL REFERENCES payment_dispatch_starts(attempt_id),
 recognition_id text NOT NULL REFERENCES payment_recognitions(id),
 group_id text NOT NULL REFERENCES basket_hold_groups(id),
 actor text NOT NULL CHECK(actor='late-stock-review'),
 reason text NOT NULL CHECK(reason IN ('component_scarcity','release_intent_unresolved','prior_consumption_without_allocation')),
 assessed_at bigint NOT NULL CHECK(assessed_at BETWEEN 0 AND 9007199254740991),
 footprint jsonb NOT NULL CHECK(jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=2097152)
);

-- Business fields only. Audit recorded_at is not a domain/commit clock and must
-- not make an immutable digest depend on the connection's TimeZone setting.
CREATE FUNCTION late_stock_movement_digest(m stock_movements) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('sequence',m.sequence,'effect_key',m.effect_key,
  'component_code',m.component_code,'action',m.action,'quantity',m.quantity,'actor',m.actor,'reason',m.reason,
  'on_hand_delta',m.on_hand_delta,'quarantine_delta',m.quarantine_delta,'reserved_delta',m.reserved_delta,
  'on_hand_after',m.on_hand_after,'quarantine_after',m.quarantine_after,'reserved_after',m.reserved_after,
  'hold_id',m.hold_id)::text,'UTF8')),'hex')
$$;
CREATE FUNCTION late_stock_digest(s late_stock_allocations) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',s.id,'order_id',s.order_id,'attempt_id',s.attempt_id,
  'recognition_id',s.recognition_id,'original_group_id',s.original_group_id,'fresh_group_id',s.fresh_group_id,
  'actor',s.actor,'allocated_at',s.allocated_at,'expires_at',s.expires_at,'footprint',s.footprint,'fence_key',s.fence_key)::text,'UTF8')),'hex')
$$;
CREATE FUNCTION late_stock_review_digest(s late_stock_review_cases) RETURNS text LANGUAGE sql IMMUTABLE STRICT AS $$
 SELECT encode(sha256(convert_to(jsonb_build_object('id',s.id,'order_id',s.order_id,'attempt_id',s.attempt_id,
  'recognition_id',s.recognition_id,'group_id',s.group_id,'actor',s.actor,'reason',s.reason,
  'assessed_at',s.assessed_at,'footprint',s.footprint,'fence_key',s.fence_key)::text,'UTF8')),'hex')
$$;

-- Keep the original settlement source proof, but inspect each bounded hold in
-- one ordered query rather than issuing separate SPI queries for every line.
CREATE OR REPLACE FUNCTION order_stock_source(wanted_order text) RETURNS jsonb LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; g basket_hold_groups%ROWTYPE; h stock_holds%ROWTYPE;
 m basket_hold_members%ROWTYPE; members jsonb := '[]'; components jsonb; n bigint := 0;
 lines bigint; actual_lines bigint; effects bigint; terminal text; total bigint := 0; problem text;
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
  terminal=CASE h.state WHEN 'released' THEN 'release-hold' WHEN 'expired' THEN 'expire-hold' WHEN 'consumed' THEN 'consume-hold' ELSE NULL END;
  WITH hold_lines AS MATERIALIZED (
   SELECT *,row_number() OVER (ORDER BY component_code) AS line_number
   FROM (SELECT * FROM stock_hold_lines WHERE hold_id=h.id ORDER BY component_code LIMIT 65) bounded
  ), checked AS (
   SELECT l.component_code,l.quantity,CASE
    WHEN total+l.line_number>1280 OR p.code IS NULL OR b.quantity::bigint*h.units IS NULL
     OR l.quantity IS DISTINCT FROM b.quantity::bigint*h.units
     THEN 'Settlement requires exact original component allocation'
    WHEN reserve.sequence IS NULL OR reserve.quantity<>l.quantity OR reserve.actor<>owned.id
     OR reserve.reason<>'sealed BOM hold transition'
     OR reserve.effect_key<>('hold:'||h.id||':reserve:'||l.component_code)
     OR (reserve.on_hand_delta,reserve.quarantine_delta,reserve.reserved_delta) IS DISTINCT FROM (0::bigint,0::bigint,l.quantity)
     OR reserve.on_hand_after<0 OR reserve.quarantine_after<0 OR reserve.reserved_after<l.quantity
     OR reserve.quarantine_after::numeric+reserve.reserved_after>reserve.on_hand_after
     THEN 'Settlement requires exact original reserve movement'
    WHEN terminal IS NOT NULL AND (ending.sequence IS NULL OR ending.sequence<=reserve.sequence OR ending.quantity<>l.quantity
     OR ending.effect_key<>('hold:'||h.id||':'||terminal||':'||l.component_code)
     OR ending.reason<>'sealed BOM hold transition' OR btrim(ending.actor)='' OR length(ending.actor)>128
     OR ending.on_hand_delta IS DISTINCT FROM (CASE WHEN terminal='consume-hold' THEN -l.quantity ELSE 0 END)
     OR ending.quarantine_delta<>0 OR ending.reserved_delta<>-l.quantity
     OR ending.on_hand_after<0 OR ending.quarantine_after<0 OR ending.reserved_after<0
     OR ending.quarantine_after::numeric+ending.reserved_after>ending.on_hand_after)
     THEN 'Terminal original hold has invalid custody' END AS problem
   FROM hold_lines l
   LEFT JOIN catalogue_bom_lines b ON b.sku=h.sku AND b.revision=h.revision AND b.component_code=l.component_code AND b.quantity BETWEEN 1 AND 10000
   LEFT JOIN physical_components p ON p.code=b.component_code AND p.kind IN ('cover','foam')
   LEFT JOIN stock_movements reserve ON reserve.hold_id=h.id AND reserve.component_code=l.component_code AND reserve.action='reserve'
   LEFT JOIN stock_movements ending ON ending.hold_id=h.id AND ending.component_code=l.component_code AND ending.action=terminal
  )
  SELECT count(*),jsonb_agg(jsonb_build_object('component_code',component_code,'quantity',quantity) ORDER BY component_code),
   (array_agg(checked.problem ORDER BY component_code) FILTER (WHERE checked.problem IS NOT NULL))[1]
   INTO actual_lines,components,problem FROM checked;
  IF actual_lines<>lines THEN RAISE EXCEPTION 'Settlement requires bounded complete BOM' USING ERRCODE='23514'; END IF;
  IF problem IS NOT NULL THEN RAISE EXCEPTION '%',problem USING ERRCODE='23514'; END IF;
  total=total+lines; effects=lines*CASE WHEN terminal IS NULL THEN 1 ELSE 2 END;
  IF (SELECT count(*) FROM (SELECT 1 FROM stock_movements WHERE hold_id=h.id LIMIT 129) bounded)<>effects THEN
   RAISE EXCEPTION 'Hold has missing or extra effects' USING ERRCODE='23514'; END IF;
  members=members||jsonb_build_array(jsonb_build_object('hold_id',h.id,'sku',h.sku,'revision',h.revision,'units',h.units,'components',components));
 END LOOP;
 RETURN members;
END;
$$;

CREATE FUNCTION late_stock_original(wanted_order text) RETURNS jsonb LANGUAGE plpgsql VOLATILE AS $$
DECLARE members jsonb; member jsonb; h stock_holds%ROWTYPE; action_name text;
 result jsonb := '[]'; pairs jsonb;
BEGIN
 members=order_stock_source(wanted_order);
 FOR member IN SELECT value FROM jsonb_array_elements(members) LOOP
  SELECT * INTO h FROM stock_holds WHERE id=member->>'hold_id';
  action_name=CASE h.state WHEN 'expired' THEN 'expire-hold' WHEN 'released' THEN 'release-hold' WHEN 'consumed' THEN 'consume-hold' ELSE NULL END;
  SELECT COALESCE(jsonb_agg(jsonb_build_object('component_code',line->>'component_code',
    'reserve_sequence',reserve.sequence,'reserve_digest',late_stock_movement_digest(reserve),
    'terminal_sequence',terminal.sequence,'terminal_digest',CASE WHEN action_name IS NULL THEN NULL ELSE late_stock_movement_digest(terminal) END)
    ORDER BY line_number),'[]'::jsonb) INTO pairs
   FROM jsonb_array_elements(member->'components') WITH ORDINALITY AS lines(line,line_number)
   LEFT JOIN stock_movements reserve ON reserve.hold_id=h.id AND reserve.component_code=line->>'component_code' AND reserve.action='reserve'
   LEFT JOIN stock_movements terminal ON terminal.hold_id=h.id AND terminal.component_code=line->>'component_code' AND terminal.action=action_name;
  result=result||jsonb_build_array(jsonb_build_object('member',member,'state',h.state,'pairs',pairs));
 END LOOP;
 RETURN result;
END;
$$;

-- Captured ACTIVE is a historical assessment, not a global sequence watermark.
-- Its exact reserve references remain; a valid later terminal transition is allowed.
CREATE FUNCTION late_stock_original_exact(wanted_order text,captured jsonb,allow_later_active boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE current_members jsonb; cm jsonb; member jsonb; h stock_holds%ROWTYPE;
 state_name text; action_name text; n integer := 0; expected_pairs jsonb;
BEGIN
 IF jsonb_typeof(captured) IS DISTINCT FROM 'array' OR jsonb_array_length(captured) NOT BETWEEN 1 AND 20
  OR octet_length(captured::text)>2097152 THEN RETURN false; END IF;
 current_members=order_stock_source(wanted_order);
 IF jsonb_array_length(current_members)<>jsonb_array_length(captured) THEN RETURN false; END IF;
 FOR cm IN SELECT value FROM jsonb_array_elements(captured) LOOP
  member=current_members->n;n=n+1;state_name=cm->>'state';
  IF (cm->'member')::text IS DISTINCT FROM member::text OR state_name IS NULL OR state_name NOT IN ('active','expired','released','consumed')
   OR jsonb_typeof(cm->'pairs') IS DISTINCT FROM 'array'
   OR jsonb_array_length(cm->'pairs')<>jsonb_array_length(member->'components') THEN RETURN false; END IF;
  SELECT * INTO h FROM stock_holds WHERE id=member->>'hold_id';
  IF state_name<>'active' AND h.state IS DISTINCT FROM state_name THEN RETURN false; END IF;
  IF state_name='active' AND NOT allow_later_active AND h.state<>'active' THEN RETURN false; END IF;
  action_name=CASE state_name WHEN 'expired' THEN 'expire-hold' WHEN 'released' THEN 'release-hold' WHEN 'consumed' THEN 'consume-hold' ELSE NULL END;
  SELECT COALESCE(jsonb_agg(jsonb_build_object('component_code',line->>'component_code',
    'reserve_sequence',reserve.sequence,'reserve_digest',late_stock_movement_digest(reserve),
    'terminal_sequence',terminal.sequence,'terminal_digest',CASE WHEN action_name IS NULL THEN NULL ELSE late_stock_movement_digest(terminal) END)
    ORDER BY line_number),'[]'::jsonb) INTO expected_pairs
   FROM jsonb_array_elements(member->'components') WITH ORDINALITY AS lines(line,line_number)
   LEFT JOIN stock_movements reserve ON reserve.hold_id=h.id AND reserve.component_code=line->>'component_code' AND reserve.action='reserve'
   LEFT JOIN stock_movements terminal ON terminal.hold_id=h.id AND terminal.component_code=line->>'component_code' AND terminal.action=action_name;
  IF (cm->'pairs')::text IS DISTINCT FROM expected_pairs::text THEN RETURN false; END IF;
  IF cm::text IS DISTINCT FROM jsonb_build_object('member',member,'state',state_name,'pairs',cm->'pairs')::text THEN RETURN false; END IF;
 END LOOP;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_received(wanted_order text,wanted_attempt text,wanted_recognition text,at_time bigint,require_current boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE r payment_recognitions%ROWTYPE; owned checkout_orders%ROWTYPE;
BEGIN
 SELECT * INTO r FROM payment_recognitions WHERE id=wanted_recognition;
 SELECT * INTO owned FROM checkout_orders WHERE id=wanted_order;
 IF r.id IS NULL OR owned.id IS NULL OR r.order_id<>owned.id OR r.attempt_id<>wanted_attempt OR r.state<>'received'
  OR at_time<r.recorded_at OR at_time<owned.created_at OR at_time NOT BETWEEN 0 AND 9007199254740991
  OR NOT payment_recognition_source(r,require_current) THEN RETURN false; END IF;
 IF require_current AND (NOT EXISTS(SELECT 1 FROM payment_recognition_current p WHERE p.order_id=owned.id AND p.recognition_id=r.id)
  OR EXISTS(SELECT 1 FROM payment_recognitions WHERE order_id=owned.id AND through_observation>r.through_observation)) THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id AND e.kind='payment.recognized.v1'
  AND e.created_at=r.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,
   'attempt_id',r.attempt_id,'recognition_id',r.id,'recognition_sha256',payment_recognition_digest(r)));
END;
$$;

-- Closed component stages; current_mode selects admission before or final after.
CREATE FUNCTION late_stock_components_exact(original jsonb,components jsonb,stages boolean,current_mode text)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE demand record; c jsonb; b jsonb; expired jsonb; reserved jsonb; final_balance jsonb;
 p bigint; q bigint; r bigint; n integer := 0; actual stock_balances%ROWTYPE; expected jsonb;
BEGIN
 IF jsonb_typeof(components) IS DISTINCT FROM 'array' OR jsonb_array_length(components) NOT BETWEEN 1 AND 1280 THEN RETURN false; END IF;
 FOR demand IN WITH demands AS MATERIALIZED (SELECT line->>'component_code' AS code,sum((line->>'quantity')::bigint)::bigint AS quantity,
   sum(CASE WHEN m->>'state'='active' THEN (line->>'quantity')::bigint ELSE 0 END)::bigint AS active
   FROM jsonb_array_elements(original) m CROSS JOIN LATERAL jsonb_array_elements(m->'member'->'components') line
   GROUP BY line->>'component_code'),
  numbered AS (SELECT *,row_number() OVER (ORDER BY code) AS demand_ordinal FROM demands),
  supplied AS MATERIALIZED (SELECT value,ordinality FROM jsonb_array_elements(components) WITH ORDINALITY)
  SELECT numbered.code,numbered.quantity,numbered.active,supplied.value AS component
   FROM numbered LEFT JOIN supplied ON supplied.ordinality=numbered.demand_ordinal ORDER BY numbered.demand_ordinal LOOP
  c=demand.component;n=n+1;b=c->'before';
  p=(b->>'on_hand')::bigint;q=(b->>'quarantine')::bigint;r=(b->>'reserved')::bigint;
  IF p IS NULL OR q IS NULL OR r IS NULL OR p<0 OR q<0 OR r<demand.active OR q::numeric+r>p
   OR demand.quantity NOT BETWEEN 1 AND 200000000 OR demand.active NOT BETWEEN 0 AND demand.quantity THEN RETURN false; END IF;
  expected=jsonb_build_object('component_code',demand.code,'demand',demand.quantity,'active',demand.active,
   'before',jsonb_build_object('on_hand',p,'quarantine',q,'reserved',r));
  IF stages THEN
   IF p::numeric-q-r+demand.active<demand.quantity THEN RETURN false; END IF;
   expired=jsonb_build_object('on_hand',p,'quarantine',q,'reserved',r-demand.active);
   reserved=jsonb_build_object('on_hand',p,'quarantine',q,'reserved',r-demand.active+demand.quantity);
   final_balance=jsonb_build_object('on_hand',p-demand.quantity,'quarantine',q,'reserved',r-demand.active);
   expected=expected||jsonb_build_object('expired',expired,'reserved',reserved,'after',final_balance);
  END IF;
  IF c::text IS DISTINCT FROM expected::text THEN RETURN false; END IF;
  IF current_mode<>'none' THEN
   SELECT * INTO actual FROM stock_balances WHERE component_code=demand.code;
   IF actual.component_code IS NULL OR (CASE current_mode WHEN 'before' THEN b WHEN 'after' THEN final_balance ELSE NULL END)
    IS DISTINCT FROM jsonb_build_object('on_hand',actual.on_hand,'quarantine',actual.quarantine,'reserved',actual.reserved) THEN RETURN false; END IF;
  END IF;
 END LOOP;
 RETURN n=jsonb_array_length(components);
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_fresh_exact(s late_stock_allocations,admission boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE g basket_hold_groups%ROWTYPE; h stock_holds%ROWTYPE; f jsonb; source_member jsonb; lines jsonb;
 fresh jsonb := s.footprint->'fresh'; count_members bigint; n integer := 0;
BEGIN
 IF jsonb_typeof(fresh) IS DISTINCT FROM 'array' OR jsonb_array_length(fresh) NOT BETWEEN 1 AND 20
  OR jsonb_array_length(fresh)<>jsonb_array_length(s.footprint->'original')
  OR (SELECT count(DISTINCT value->>'hold_id') FROM jsonb_array_elements(fresh))<>jsonb_array_length(fresh)
  OR (SELECT count(DISTINCT value->>'sku') FROM jsonb_array_elements(fresh))<>jsonb_array_length(fresh) THEN RETURN false; END IF;
 SELECT * INTO g FROM basket_hold_groups WHERE id=s.fresh_group_id;
 IF g.id IS NULL OR g.id=s.original_group_id OR g.owner_id<>s.order_id
  OR g.created_at<>s.expires_at-60 OR g.expires_at<>s.expires_at OR g.members_count<>jsonb_array_length(fresh)
  OR g.sealed=admission
  OR (SELECT count(*) FROM (SELECT 1 FROM stock_holds WHERE basket_group_id=g.id LIMIT 21) bounded)<>g.members_count THEN RETURN false; END IF;
 SELECT count(*) INTO count_members FROM (SELECT 1 FROM basket_hold_members WHERE group_id=g.id LIMIT 21) bounded;
 IF (admission AND count_members<>0) OR (NOT admission AND count_members<>g.members_count) THEN RETURN false; END IF;
 FOR f IN SELECT value FROM jsonb_array_elements(fresh) LOOP
  n=n+1;SELECT * INTO h FROM stock_holds WHERE id=f->>'hold_id';
  SELECT value->'member' INTO source_member FROM jsonb_array_elements(s.footprint->'original') WHERE value->'member'->>'sku'=f->>'sku';
  IF h.id IS NULL OR h.id !~ '^[a-f0-9]{32}$' OR source_member IS NULL OR h.basket_group_id IS DISTINCT FROM g.id
   OR (h.owner_id,h.created_at,h.expires_at,h.sku,h.revision,h.units)
    IS DISTINCT FROM (g.owner_id,g.created_at,g.expires_at,source_member->>'sku',(source_member->>'revision')::integer,(source_member->>'units')::integer)
   OR h.state IS DISTINCT FROM (CASE WHEN admission THEN 'building' ELSE 'consumed' END)
   OR f::text IS DISTINCT FROM jsonb_set(source_member,'{hold_id}',to_jsonb(h.id))::text
   OR EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') m WHERE m->'member'->>'hold_id'=h.id) THEN RETURN false; END IF;
  IF admission THEN
   IF EXISTS(SELECT 1 FROM stock_hold_lines WHERE hold_id=h.id)
    OR EXISTS(SELECT 1 FROM stock_movements WHERE hold_id=h.id) THEN RETURN false; END IF;
  ELSE
   SELECT jsonb_agg(jsonb_build_object('component_code',component_code,'quantity',quantity) ORDER BY component_code)
    INTO lines FROM (SELECT * FROM stock_hold_lines WHERE hold_id=h.id ORDER BY component_code LIMIT 65) bounded;
   IF lines IS DISTINCT FROM f->'components'
    OR NOT EXISTS(SELECT 1 FROM basket_hold_members m WHERE m.hold_id=h.id AND m.group_id=g.id
      AND (m.sku,m.revision,m.units)=(h.sku,h.revision,h.units))
    OR (SELECT count(*) FROM (SELECT 1 FROM stock_movements WHERE hold_id=h.id LIMIT 129) bounded)<>2*jsonb_array_length(f->'components')
   THEN RETURN false; END IF;
  END IF;
 END LOOP;
 RETURN n=g.members_count;
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_stage_exact(s late_stock_allocations,stage text)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE violation boolean; original jsonb := s.footprint->'original'; fresh jsonb := s.footprint->'fresh';
 component_footprint jsonb := s.footprint->'components';
BEGIN
 IF stage IS NULL OR stage NOT IN ('expiry','reserve','consume','all') THEN RETURN false; END IF;
 WITH phases AS (
  SELECT phase,action_name,actor_name FROM (VALUES
   ('expiry','expire-hold','late-payment-expiry'),
   ('reserve','reserve',s.order_id),
   ('consume','consume-hold','payment-settlement')) AS p(phase,action_name,actor_name)
   WHERE stage='all' OR phase=stage
 ), members AS (
  SELECT phases.*,CASE WHEN phase='expiry' THEN value->'member' ELSE value END AS member,ordinality AS member_number
   FROM phases CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN phase='expiry' THEN original ELSE fresh END) WITH ORDINALITY
   WHERE phase<>'expiry' OR value->>'state'='active'
 ), pairs AS MATERIALIZED (
  SELECT phase,action_name,actor_name,member->>'hold_id' AS hold_id,line->>'component_code' AS code,(line->>'quantity')::bigint AS quantity,
   member_number,line_number
  FROM members CROSS JOIN LATERAL jsonb_array_elements(member->'components') WITH ORDINALITY AS lines(line,line_number)
 ), hold_ids AS MATERIALIZED (
  SELECT DISTINCT hold_id FROM pairs
 ), movements AS MATERIALIZED (
  -- Parameterized hold lookups avoid scanning unrelated ledger history.
  SELECT movement.* FROM hold_ids ids CROSS JOIN LATERAL (
   SELECT m.hold_id,m.component_code,m.action,m.sequence,m.effect_key,m.actor,m.reason,m.quantity,
    m.on_hand_delta,m.quarantine_delta,m.reserved_delta,m.on_hand_after,m.quarantine_after,m.reserved_after
   FROM stock_movements m WHERE m.hold_id=ids.hold_id AND m.action IN ('expire-hold','reserve','consume-hold') OFFSET 0
  ) movement
 ), pair_codes AS MATERIALIZED (
  SELECT DISTINCT code FROM pairs
 ), components AS MATERIALIZED (
  -- Decode only referenced component rows, once for all requested stages.
  SELECT value->>'component_code' AS code,(value->'before'->>'on_hand')::bigint AS p,
   (value->'before'->>'quarantine')::bigint AS q,(value->'before'->>'reserved')::bigint AS r,
   (value->>'active')::bigint AS active,(value->>'demand')::bigint AS demand
  FROM jsonb_array_elements(component_footprint) JOIN pair_codes ON pair_codes.code=value->>'component_code'
 ), measured AS (
  SELECT pairs.*,movement.sequence,movement.effect_key,movement.action,movement.actor,movement.reason,
   movement.quantity AS actual_quantity,movement.on_hand_delta,movement.quarantine_delta,movement.reserved_delta,
   movement.on_hand_after,movement.quarantine_after,movement.reserved_after,
   sum(pairs.quantity) OVER(PARTITION BY pairs.phase,pairs.code ORDER BY member_number,line_number) AS progressed,
   lag(movement.sequence) OVER(PARTITION BY pairs.phase ORDER BY member_number,line_number) AS previous,
   component.p,component.q,component.r,component.active,component.demand
  FROM pairs LEFT JOIN movements movement ON movement.hold_id=pairs.hold_id
    AND movement.component_code=pairs.code AND movement.action=pairs.action_name
   LEFT JOIN components component ON component.code=pairs.code
 )
 SELECT EXISTS(SELECT 1 FROM measured WHERE sequence IS NULL OR actual_quantity IS DISTINCT FROM quantity
  OR actor IS DISTINCT FROM actor_name OR reason IS DISTINCT FROM 'sealed BOM hold transition'
  OR effect_key IS DISTINCT FROM ('hold:'||hold_id||':'||action_name||':'||code)
  OR (previous IS NOT NULL AND sequence<=previous)
  OR on_hand_delta IS DISTINCT FROM (CASE WHEN phase='consume' THEN -quantity ELSE 0 END)
  OR quarantine_delta IS DISTINCT FROM 0::bigint
  OR reserved_delta IS DISTINCT FROM (CASE WHEN phase='reserve' THEN quantity ELSE -quantity END)
  OR on_hand_after IS DISTINCT FROM (CASE WHEN phase='consume' THEN p-progressed ELSE p END)
  OR quarantine_after IS DISTINCT FROM q
  OR reserved_after IS DISTINCT FROM (CASE phase WHEN 'expiry' THEN r-progressed WHEN 'reserve' THEN r-active+progressed ELSE r-active+demand-progressed END)) INTO violation;
 RETURN NOT violation;
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_allocation_exact(s late_stock_allocations,current_balances boolean,current_financial boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; m jsonb; fresh_ids text[]; active_ids text[];
 last_expiry bigint; first_reserve bigint; last_reserve bigint; first_consume bigint;
BEGIN
 IF octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF s.id IS NULL OR s.actor IS DISTINCT FROM 'late-payment-allocation' OR (s.fence_key IS NULL OR s.fence_key<=0)
  OR (s.footprint->'schema_version')::text IS DISTINCT FROM '1'
  OR s.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'original',s.footprint->'original',
    'fresh',s.footprint->'fresh','components',s.footprint->'components') THEN RETURN false; END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=s.order_id;
 IF owned.id IS NULL OR owned.stock_group_id IS DISTINCT FROM s.original_group_id OR s.original_group_id=s.fresh_group_id
  OR s.expires_at NOT BETWEEN 1 AND 9007199254740991 OR s.allocated_at<s.expires_at-60 OR s.allocated_at>=s.expires_at
  OR s.expires_at-60<owned.hold_expires_at
  OR NOT late_stock_received(s.order_id,s.attempt_id,s.recognition_id,s.expires_at-60,current_financial)
  OR NOT late_stock_original_exact(s.order_id,s.footprint->'original',true)
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state' NOT IN ('active','expired'))
  OR EXISTS(SELECT 1 FROM stock_holds WHERE basket_group_id=s.original_group_id AND state<>'expired')
  OR NOT late_stock_fresh_exact(s,false)
  OR NOT late_stock_components_exact(s.footprint->'original',s.footprint->'components',true,CASE WHEN current_balances THEN 'after' ELSE 'none' END)
  OR NOT EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=s.order_id AND a.attempt_id=s.attempt_id AND a.kind='late' AND a.receipt_id=s.id)
  OR EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=s.order_id)
  OR NOT late_stock_stage_exact(s,'all')
 THEN RETURN false; END IF;
 SELECT array_agg(value->>'hold_id') INTO fresh_ids FROM jsonb_array_elements(s.footprint->'fresh');
 SELECT array_agg(value->'member'->>'hold_id') INTO active_ids FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state'='active';
 SELECT max(sequence) INTO last_expiry FROM stock_movements WHERE hold_id=ANY(active_ids) AND action='expire-hold';
 SELECT min(sequence),max(sequence) INTO first_reserve,last_reserve FROM stock_movements WHERE hold_id=ANY(fresh_ids) AND action='reserve';
 SELECT min(sequence) INTO first_consume FROM stock_movements WHERE hold_id=ANY(fresh_ids) AND action='consume-hold';
 IF first_reserve IS NULL OR first_consume IS NULL OR (last_expiry IS NOT NULL AND first_reserve<=last_expiry)
  OR first_consume<=last_reserve THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION late_stock_review_exact(s late_stock_review_cases,admission boolean)
 RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; expected_reason text; scarce boolean;
BEGIN
 IF octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF s.id IS NULL OR s.actor IS DISTINCT FROM 'late-stock-review' OR (s.fence_key IS NULL OR s.fence_key<=0)
  OR octet_length(s.footprint::text)>2097152 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' OR (s.footprint->'schema_version')::text IS DISTINCT FROM '1'
  OR s.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'original',s.footprint->'original','components',s.footprint->'components') THEN RETURN false; END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=s.order_id;
 IF owned.id IS NULL OR owned.stock_group_id IS DISTINCT FROM s.group_id
  OR NOT late_stock_received(s.order_id,s.attempt_id,s.recognition_id,s.assessed_at,admission)
  OR NOT late_stock_original_exact(s.order_id,s.footprint->'original',NOT admission)
  OR NOT late_stock_components_exact(s.footprint->'original',s.footprint->'components',false,CASE WHEN admission THEN 'before' ELSE 'none' END)
  OR EXISTS(SELECT 1 FROM order_stock_allocations WHERE order_id=s.order_id) THEN RETURN false; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state'='consumed') THEN
  expected_reason='prior_consumption_without_allocation';
 ELSIF EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'original') WHERE value->>'state'='released') THEN
  expected_reason='release_intent_unresolved';
 ELSE
  IF s.assessed_at<owned.hold_expires_at THEN RETURN false; END IF;
  SELECT EXISTS(SELECT 1 FROM jsonb_array_elements(s.footprint->'components') c
   WHERE (c->'before'->>'on_hand')::numeric-(c->'before'->>'quarantine')::bigint-(c->'before'->>'reserved')::bigint+(c->>'active')::bigint<(c->>'demand')::bigint) INTO scarce;
  IF NOT scarce THEN RETURN false; END IF;expected_reason='component_scarcity';
 END IF;
 IF s.reason IS DISTINCT FROM expected_reason THEN RETURN false; END IF;
 IF admission AND (s.footprint->'original')::text IS DISTINCT FROM late_stock_original(s.order_id)::text THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION order_stock_original_history_exact(s order_stock_settlements) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; members jsonb; c jsonb; demand record; n integer := 0; p bigint; q bigint; r bigint;
BEGIN
 IF octet_length(s.footprint::text)>1048576 OR jsonb_typeof(s.footprint) IS DISTINCT FROM 'object' THEN RETURN false; END IF;
 IF s.id IS NULL OR s.actor IS DISTINCT FROM 'payment-settlement' OR (s.fence_key IS NULL OR s.fence_key<=0)
  OR octet_length(s.footprint::text)>1048576 OR jsonb_typeof(s.footprint->'components') IS DISTINCT FROM 'array'
  OR s.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'members',s.footprint->'members','components',s.footprint->'components') THEN RETURN false; END IF;
 SELECT * INTO owned FROM checkout_orders WHERE id=s.order_id;
 IF owned.id IS NULL OR s.group_id IS DISTINCT FROM owned.stock_group_id OR s.settled_at>=owned.hold_expires_at
  OR NOT late_stock_received(s.order_id,s.attempt_id,s.recognition_id,s.settled_at,false)
  OR NOT order_stock_effects_exact(s,false) THEN RETURN false; END IF;
 members=s.footprint->'members';
 IF (s.footprint->'schema_version')::text IS DISTINCT FROM '1' OR EXISTS(SELECT 1 FROM jsonb_array_elements(members) m WHERE (m->'revision')::text !~ '^[1-9][0-9]*$' OR (m->'units')::text !~ '^[1-9][0-9]*$' OR EXISTS(SELECT 1 FROM jsonb_array_elements(m->'components') l WHERE (l->'quantity')::text !~ '^[1-9][0-9]*$')) THEN RETURN false; END IF;
 FOR demand IN SELECT line->>'component_code' AS code,sum((line->>'quantity')::bigint)::bigint AS quantity
  FROM jsonb_array_elements(members) m CROSS JOIN LATERAL jsonb_array_elements(m->'components') line
  GROUP BY line->>'component_code' ORDER BY line->>'component_code' LOOP
  c=s.footprint->'components'->n;n=n+1;p=(c->'before'->>'on_hand')::bigint;q=(c->'before'->>'quarantine')::bigint;r=(c->'before'->>'reserved')::bigint;
  IF n>1280 OR p IS NULL OR q IS NULL OR r IS NULL OR p<0 OR q<0 OR r<demand.quantity OR q::numeric+r>p
   OR c::text IS DISTINCT FROM jsonb_build_object('component_code',demand.code,'quantity',demand.quantity,
    'before',jsonb_build_object('on_hand',p,'quarantine',q,'reserved',r),
    'after',jsonb_build_object('on_hand',p-demand.quantity,'quarantine',q,'reserved',r-demand.quantity))::text THEN RETURN false; END IF;
 END LOOP;
 IF n<>jsonb_array_length(s.footprint->'components') THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_settled.v1'
  AND e.created_at=s.settled_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,
   'settlement_id',s.id,'settlement_sha256',order_stock_digest(s)));
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch THEN RETURN false;
END;
$$;

CREATE FUNCTION guard_order_stock_allocation_claim() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Order allocation claims are immutable' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Order allocation requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 IF EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=NEW.order_id)
  OR (NEW.kind='original' AND NOT EXISTS(SELECT 1 FROM order_stock_settlements s WHERE s.id=NEW.receipt_id AND s.order_id=NEW.order_id AND s.attempt_id=NEW.attempt_id))
  OR (NEW.kind='late' AND NOT EXISTS(SELECT 1 FROM late_stock_allocations s WHERE s.id=NEW.receipt_id AND s.order_id=NEW.order_id AND s.attempt_id=NEW.attempt_id)) THEN
  RAISE EXCEPTION 'Allocation claim requires exact owning receipt and no review stop' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_order_stock_allocation_claim BEFORE INSERT OR UPDATE OR DELETE ON order_stock_allocations FOR EACH ROW EXECUTE FUNCTION guard_order_stock_allocation_claim();

CREATE FUNCTION order_stock_allocation_exact(a order_stock_allocations) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE original order_stock_settlements%ROWTYPE; late late_stock_allocations%ROWTYPE;
BEGIN
 IF a.order_id IS NULL OR EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=a.order_id) THEN RETURN false; END IF;
 IF a.kind='original' THEN
  SELECT * INTO original FROM order_stock_settlements WHERE id=a.receipt_id AND order_id=a.order_id AND attempt_id=a.attempt_id;
  RETURN order_stock_original_history_exact(original)
   AND NOT EXISTS(SELECT 1 FROM late_stock_allocations WHERE order_id=a.order_id);
 ELSIF a.kind='late' THEN
  SELECT * INTO late FROM late_stock_allocations WHERE id=a.receipt_id AND order_id=a.order_id AND attempt_id=a.attempt_id;
  IF NOT late_stock_allocation_exact(late,false,false) THEN RETURN false; END IF;
  RETURN NOT EXISTS(SELECT 1 FROM order_stock_settlements WHERE order_id=a.order_id)
   AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=late.order_id AND e.kind='order.stock_late_allocated.v1'
    AND e.created_at=late.allocated_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',late.order_id,
     'allocation_id',late.id,'allocation_sha256',late_stock_digest(late)));
 END IF;
 RETURN false;
END;
$$;
CREATE FUNCTION require_complete_order_stock_allocation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF NOT order_stock_allocation_exact(NEW) THEN
  RAISE EXCEPTION 'Allocation claim requires complete reciprocal receipt and event' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_order_stock_allocation AFTER INSERT ON order_stock_allocations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_order_stock_allocation();

-- Existing history is certified by historical financial/source/effect custody,
-- never by the current pointer. A corrupt row aborts the entire migration.
DO $$
DECLARE original order_stock_settlements%ROWTYPE;
BEGIN
 IF EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.kind='order.stock_settled.v1' AND NOT EXISTS(
  SELECT 1 FROM order_stock_settlements s WHERE s.id=e.payload->>'settlement_id' AND s.order_id=e.aggregate_id)) THEN
  RAISE EXCEPTION 'Cannot backfill orphan original stock event' USING ERRCODE='23514'; END IF;
 FOR original IN SELECT * FROM order_stock_settlements ORDER BY order_id LOOP
  IF NOT order_stock_original_history_exact(original) THEN
   RAISE EXCEPTION 'Cannot backfill damaged original stock allocation history' USING ERRCODE='23514'; END IF;
  INSERT INTO order_stock_allocations(order_id,attempt_id,kind,receipt_id) VALUES(original.order_id,original.attempt_id,'original',original.id);
 END LOOP;
END;
$$;

CREATE FUNCTION claim_owning_order_stock_allocation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 INSERT INTO order_stock_allocations(order_id,attempt_id,kind,receipt_id)
  VALUES(NEW.order_id,NEW.attempt_id,CASE WHEN TG_TABLE_NAME='order_stock_settlements' THEN 'original' ELSE 'late' END,NEW.id);
 RETURN NULL;
END;
$$;
CREATE TRIGGER claim_original_order_stock_allocation AFTER INSERT ON order_stock_settlements FOR EACH ROW EXECUTE FUNCTION claim_owning_order_stock_allocation();
CREATE TRIGGER claim_late_order_stock_allocation AFTER INSERT ON late_stock_allocations FOR EACH ROW EXECUTE FUNCTION claim_owning_order_stock_allocation();

CREATE FUNCTION guard_late_stock_allocation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; original jsonb;
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Late allocations are immutable' USING ERRCODE='23514'; END IF;
 IF octet_length(NEW.footprint::text)>2097152 OR jsonb_typeof(NEW.footprint) IS DISTINCT FROM 'object' THEN
  RAISE EXCEPTION 'Late allocation footprint exceeds closed envelope' USING ERRCODE='23514'; END IF;
 original=NEW.footprint->'original';
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Late allocation requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 SELECT * INTO owned FROM checkout_orders WHERE id=NEW.order_id;
 IF owned.id IS NULL OR owned.stock_group_id IS DISTINCT FROM NEW.original_group_id
  OR NEW.expires_at NOT BETWEEN 1 AND 9007199254740991
  OR NEW.allocated_at NOT BETWEEN 0 AND 9007199254740991
  OR NEW.allocated_at<NEW.expires_at-60 OR NEW.allocated_at>=NEW.expires_at
  OR NEW.expires_at-60<owned.hold_expires_at
  OR NOT late_stock_received(NEW.order_id,NEW.attempt_id,NEW.recognition_id,NEW.expires_at-60,true)
  OR EXISTS(SELECT 1 FROM order_stock_allocations WHERE order_id=NEW.order_id)
  OR EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=NEW.order_id) THEN
  RAISE EXCEPTION 'Late allocation requires current received source and no prior outcome' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.original_group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.original_group_id ORDER BY id LIMIT 21 FOR UPDATE;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.fresh_group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.fresh_group_id ORDER BY id LIMIT 21 FOR UPDATE;
 IF (NEW.footprint->'schema_version')::text IS DISTINCT FROM '1' OR NEW.footprint IS DISTINCT FROM jsonb_build_object('schema_version',1,'original',NEW.footprint->'original',
   'fresh',NEW.footprint->'fresh','components',NEW.footprint->'components')
  OR original::text IS DISTINCT FROM late_stock_original(NEW.order_id)::text
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(original) WHERE value->>'state' NOT IN ('active','expired'))
  OR NOT late_stock_fresh_exact(NEW,true) THEN
  RAISE EXCEPTION 'Late allocation requires exact eligible original and fresh building source' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM stock_balances b JOIN (
  WITH wanted_codes AS MATERIALIZED (
   SELECT DISTINCT line->>'component_code' AS code FROM jsonb_array_elements(original) m
    CROSS JOIN LATERAL jsonb_array_elements(m->'member'->'components') line)
  SELECT code FROM wanted_codes
 ) wanted ON wanted.code=b.component_code
  ORDER BY b.component_code LIMIT 1281 FOR UPDATE OF b;
 IF NOT late_stock_components_exact(original,NEW.footprint->'components',true,'before') THEN
  RAISE EXCEPTION 'Late allocation requires actual backed before and complete stage footprint' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_late_stock_allocation BEFORE INSERT OR UPDATE OR DELETE ON late_stock_allocations FOR EACH ROW EXECUTE FUNCTION guard_late_stock_allocation();

CREATE FUNCTION guard_late_stock_review() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF TG_OP<>'INSERT' THEN RAISE EXCEPTION 'Stock review cases are immutable' USING ERRCODE='23514'; END IF;
 IF octet_length(NEW.footprint::text)>2097152 OR jsonb_typeof(NEW.footprint) IS DISTINCT FROM 'object' THEN
  RAISE EXCEPTION 'Stock review footprint exceeds closed envelope' USING ERRCODE='23514'; END IF;
 IF current_setting('transaction_isolation')<>'read committed' THEN RAISE EXCEPTION 'Stock review requires read committed' USING ERRCODE='23514'; END IF;
 PERFORM 1 FROM payment_attempts WHERE id=NEW.attempt_id FOR UPDATE;
 PERFORM 1 FROM basket_hold_groups WHERE id=NEW.group_id FOR SHARE;
 PERFORM 1 FROM stock_holds WHERE basket_group_id=NEW.group_id ORDER BY id LIMIT 21 FOR UPDATE;
 PERFORM 1 FROM stock_balances WHERE component_code IN (
  SELECT line->>'component_code' FROM jsonb_array_elements(order_stock_source(NEW.order_id)) m
   CROSS JOIN LATERAL jsonb_array_elements(m->'components') line)
  ORDER BY component_code LIMIT 1281 FOR UPDATE;
 IF NOT late_stock_review_exact(NEW,true) THEN
  RAISE EXCEPTION 'Stock review requires exact received historical assessment and actual source' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END;
$$;
CREATE TRIGGER guard_late_stock_review BEFORE INSERT OR UPDATE OR DELETE ON late_stock_review_cases FOR EACH ROW EXECUTE FUNCTION guard_late_stock_review();

CREATE FUNCTION require_complete_late_stock_allocation() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF octet_length(NEW.footprint::text)>2097152 THEN RAISE EXCEPTION 'Stored stock outcome exceeds bounded envelope' USING ERRCODE='23514'; END IF;
 IF NOT late_stock_allocation_exact(NEW,true,true)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=NEW.order_id AND e.kind='order.stock_late_allocated.v1'
   AND e.created_at=NEW.allocated_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',NEW.order_id,
    'allocation_id',NEW.id,'allocation_sha256',late_stock_digest(NEW))) THEN
  RAISE EXCEPTION 'Late allocation requires complete exact physical stages claim and event' USING ERRCODE='23514'; END IF;
 -- Full proof always precedes the seal, even if a caller owns a matching lock.
 PERFORM pg_advisory_xact_lock(1937011564,NEW.fence_key);
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_late_stock_allocation AFTER INSERT ON late_stock_allocations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_late_stock_allocation();
CREATE FUNCTION require_complete_late_stock_review() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF octet_length(NEW.footprint::text)>2097152 THEN RAISE EXCEPTION 'Stored stock outcome exceeds bounded envelope' USING ERRCODE='23514'; END IF;
 IF NOT late_stock_review_exact(NEW,false)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=NEW.order_id AND e.kind='order.stock_recovery_review.v1'
   AND e.created_at=NEW.assessed_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',NEW.order_id,
    'case_id',NEW.id,'case_sha256',late_stock_review_digest(NEW))) THEN
  RAISE EXCEPTION 'Stock review requires complete historical assessment and event' USING ERRCODE='23514'; END IF;
 RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_complete_late_stock_review AFTER INSERT ON late_stock_review_cases DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_complete_late_stock_review();

CREATE OR REPLACE FUNCTION require_complete_order_stock_settlement() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
BEGIN
 IF octet_length(NEW.footprint::text)>1048576 THEN RAISE EXCEPTION 'Stored stock outcome exceeds bounded envelope' USING ERRCODE='23514'; END IF;
 IF NOT order_stock_received(NEW) OR NOT order_stock_effects_exact(NEW,true)
  OR NOT EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=NEW.order_id AND a.attempt_id=NEW.attempt_id AND a.kind='original' AND a.receipt_id=NEW.id)
  OR EXISTS(SELECT 1 FROM late_stock_review_cases WHERE order_id=NEW.order_id)
  OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=NEW.order_id AND e.kind='order.stock_settled.v1'
   AND e.created_at=NEW.settled_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',NEW.order_id,
    'settlement_id',NEW.id,'settlement_sha256',order_stock_digest(NEW))) THEN
  RAISE EXCEPTION 'Settlement requires complete exact physical effects and event' USING ERRCODE='23514'; END IF;
 PERFORM pg_advisory_xact_lock(1937011563,NEW.fence_key);
 RETURN NULL;
END;
$$;

ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_kind_check;
ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_kind_check CHECK(kind IN ('checkout.order_created.v1','payment.attempt_prepared.v1','payment.reconciliation_required.v1','payment.observation_recorded.v1','payment.search_page_recorded.v1','payment.identity_resolved.v1','payment.recognized.v1','order.stock_settled.v1','order.stock_late_allocated.v1','order.stock_recovery_review.v1'));

CREATE OR REPLACE FUNCTION guard_commerce_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE recognition payment_recognitions%ROWTYPE; settlement order_stock_settlements%ROWTYPE;
 allocation late_stock_allocations%ROWTYPE; review late_stock_review_cases%ROWTYPE;
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
  IF settlement.id IS NULL OR octet_length(settlement.footprint::text)>1048576 THEN
   RAISE EXCEPTION 'Settlement event requires bounded owning receipt' USING ERRCODE='23514'; END IF;
  IF settlement.id IS NULL OR NEW.aggregate_id IS DISTINCT FROM settlement.order_id
   OR NEW.created_at IS DISTINCT FROM settlement.settled_at
   OR NOT order_stock_effects_exact(settlement,false)
   OR NOT EXISTS(SELECT 1 FROM order_stock_allocations a WHERE a.order_id=settlement.order_id AND a.attempt_id=settlement.attempt_id AND a.kind='original' AND a.receipt_id=settlement.id)
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',settlement.order_id,
     'settlement_id',settlement.id,'settlement_sha256',order_stock_digest(settlement)) THEN
   RAISE EXCEPTION 'Settlement event requires exact immutable receipt and effects' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_late_allocated.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Late allocation event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO allocation FROM late_stock_allocations WHERE id=NEW.payload->>'allocation_id';
  PERFORM 1 FROM payment_attempts WHERE id=allocation.attempt_id FOR UPDATE;
  IF allocation.id IS NULL OR octet_length(allocation.footprint::text)>2097152 THEN
   RAISE EXCEPTION 'Late allocation event requires bounded owning receipt' USING ERRCODE='23514'; END IF;
  IF NOT late_stock_allocation_exact(allocation,false,false)
   OR NEW.aggregate_id IS DISTINCT FROM allocation.order_id OR NEW.created_at IS DISTINCT FROM allocation.allocated_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',allocation.order_id,
    'allocation_id',allocation.id,'allocation_sha256',late_stock_digest(allocation)) THEN
   RAISE EXCEPTION 'Late allocation event requires exact complete receipt and effects' USING ERRCODE='23514'; END IF;
 ELSIF NEW.kind='order.stock_recovery_review.v1' THEN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'Stock review event requires read committed' USING ERRCODE='23514'; END IF;
  SELECT * INTO review FROM late_stock_review_cases WHERE id=NEW.payload->>'case_id';
  PERFORM 1 FROM payment_attempts WHERE id=review.attempt_id FOR UPDATE;
  IF review.id IS NULL OR octet_length(review.footprint::text)>2097152 THEN
   RAISE EXCEPTION 'Stock review event requires bounded owning receipt' USING ERRCODE='23514'; END IF;
  IF NOT late_stock_review_exact(review,false)
   OR NEW.aggregate_id IS DISTINCT FROM review.order_id OR NEW.created_at IS DISTINCT FROM review.assessed_at
   OR NEW.payload IS DISTINCT FROM jsonb_build_object('schema_version',1,'order_id',review.order_id,
    'case_id',review.id,'case_sha256',late_stock_review_digest(review)) THEN
   RAISE EXCEPTION 'Stock review event requires exact historical assessment' USING ERRCODE='23514'; END IF;
 ELSE RAISE EXCEPTION 'Unknown commerce event kind' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;

-- Live backend locks drive indexed receipt lookup, never historical table scans.
-- Review stops certify admission only and intentionally do not seal mutable stock.
CREATE OR REPLACE FUNCTION guard_order_stock_completion_seal() RETURNS trigger LANGUAGE plpgsql VOLATILE AS $$
DECLARE seal record; original order_stock_settlements%ROWTYPE; late late_stock_allocations%ROWTYPE;
 attempts text[] := '{}'; orders text[] := '{}'; holds text[] := '{}'; components text[] := '{}'; groups text[] := '{}';
 footprint jsonb; receipt_attempt text; receipt_order text; receipt_groups text[]; member_match boolean;
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
 ELSIF TG_TABLE_NAME IN ('stock_movements','stock_hold_lines') THEN
  IF TG_OP<>'DELETE' THEN holds=array_append(holds,NEW.hold_id); components=array_append(components,NEW.component_code); END IF;
  IF TG_OP<>'INSERT' THEN holds=array_append(holds,OLD.hold_id); components=array_append(components,OLD.component_code); END IF;
 ELSIF TG_TABLE_NAME='basket_hold_members' THEN
  IF TG_OP<>'DELETE' THEN holds=array_append(holds,NEW.hold_id); groups=array_append(groups,NEW.group_id); END IF;
  IF TG_OP<>'INSERT' THEN holds=array_append(holds,OLD.hold_id); groups=array_append(groups,OLD.group_id); END IF;
 ELSIF TG_TABLE_NAME='basket_hold_groups' THEN
  IF TG_OP<>'DELETE' THEN groups=array_append(groups,NEW.id); END IF;
  IF TG_OP<>'INSERT' THEN groups=array_append(groups,OLD.id); END IF;
 END IF;
 FOR seal IN SELECT classid::bigint AS namespace,objid::bigint AS key FROM pg_locks
  WHERE locktype='advisory' AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
   AND pid=pg_backend_pid() AND mode='ExclusiveLock' AND granted AND classid IN (1937011563::oid,1937011564::oid) AND objsubid=2 LOOP
  IF seal.namespace=1937011563 THEN
   SELECT * INTO original FROM order_stock_settlements WHERE fence_key=seal.key;
   IF original.id IS NULL THEN CONTINUE; END IF;
   footprint=original.footprint;receipt_attempt=original.attempt_id;receipt_order=original.order_id;receipt_groups=ARRAY[original.group_id];
   IF octet_length(footprint::text)>1048576 THEN RAISE EXCEPTION 'Sealed settlement has invalid bounded custody' USING ERRCODE='23514'; END IF;
   SELECT EXISTS(SELECT 1 FROM jsonb_array_elements(footprint->'members') m WHERE m->>'hold_id'=ANY(holds)) INTO member_match;
  ELSE
   SELECT * INTO late FROM late_stock_allocations WHERE fence_key=seal.key;
   IF late.id IS NULL THEN CONTINUE; END IF;
   footprint=late.footprint;receipt_attempt=late.attempt_id;receipt_order=late.order_id;receipt_groups=ARRAY[late.original_group_id,late.fresh_group_id];
   IF octet_length(footprint::text)>2097152 THEN RAISE EXCEPTION 'Sealed late allocation has invalid bounded custody' USING ERRCODE='23514'; END IF;
   SELECT EXISTS(SELECT 1 FROM jsonb_array_elements(footprint->'original') m WHERE m->'member'->>'hold_id'=ANY(holds))
    OR EXISTS(SELECT 1 FROM jsonb_array_elements(footprint->'fresh') m WHERE m->>'hold_id'=ANY(holds)) INTO member_match;
  END IF;
  IF receipt_attempt=ANY(attempts) OR receipt_order=ANY(orders) OR receipt_groups&&groups OR member_match
   OR EXISTS(SELECT 1 FROM jsonb_array_elements(footprint->'components') c WHERE c->>'component_code'=ANY(components)) THEN
   IF seal.namespace=1937011563 THEN RAISE EXCEPTION 'Completed settlement source cannot change again in this transaction' USING ERRCODE='23514';
   ELSE RAISE EXCEPTION 'Completed late allocation source cannot change again in this transaction' USING ERRCODE='23514'; END IF;
  END IF;
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;RETURN NEW;
END;
$$;
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON basket_hold_groups FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON basket_hold_members FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
CREATE TRIGGER aa_order_stock_seal BEFORE INSERT OR UPDATE OR DELETE ON stock_hold_lines FOR EACH ROW EXECUTE FUNCTION guard_order_stock_completion_seal();
