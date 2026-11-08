-- Physical custody uses original quantities and BOMs. Financial custody uses captured payable cents.
CREATE OR REPLACE FUNCTION order_stock_source(wanted_order text) RETURNS jsonb LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; g basket_hold_groups%ROWTYPE; h stock_holds%ROWTYPE;
 m basket_hold_members%ROWTYPE; members jsonb := '[]'; components jsonb; n bigint := 0;
 quote_lines jsonb; lines bigint; actual_lines bigint; effects bigint; terminal text; total bigint := 0; problem text;
BEGIN
 SELECT * INTO owned FROM checkout_orders WHERE id=wanted_order;
 quote_lines=CASE owned.snapshot->'schema_version'
  WHEN '1'::jsonb THEN owned.snapshot->'quote'->'lines'
  WHEN '2'::jsonb THEN owned.snapshot->'quote'->'original'->'lines' END;
 SELECT * INTO g FROM basket_hold_groups WHERE id=owned.stock_group_id;
 IF owned.id IS NULL OR g.id IS NULL OR NOT g.sealed OR g.owner_id<>owned.id
  OR g.created_at<>owned.created_at OR g.expires_at<>owned.hold_expires_at
  OR g.members_count NOT BETWEEN 1 AND 20
  OR (SELECT count(*) FROM (SELECT 1 FROM basket_hold_members WHERE group_id=g.id LIMIT 21) bounded)<>g.members_count
  OR (SELECT count(*) FROM (SELECT 1 FROM stock_holds WHERE basket_group_id=g.id LIMIT 21) bounded)<>g.members_count
  OR jsonb_typeof(quote_lines) IS DISTINCT FROM 'array'
  OR jsonb_array_length(quote_lines)<>g.members_count THEN
  RAISE EXCEPTION 'Settlement requires complete original group' USING ERRCODE='23514'; END IF;
 FOR m IN SELECT * FROM basket_hold_members WHERE group_id=g.id ORDER BY hold_id LIMIT 21 LOOP
  n=n+1; SELECT * INTO h FROM stock_holds WHERE id=m.hold_id;
  IF n>20 OR h.id IS NULL OR h.id !~ '^[a-f0-9]{32}$' OR h.basket_group_id IS DISTINCT FROM g.id
   OR (h.owner_id,h.sku,h.revision,h.units,h.created_at,h.expires_at)
     IS DISTINCT FROM (g.owner_id,m.sku,m.revision,m.units,g.created_at,g.expires_at)
   OR h.units NOT BETWEEN 1 AND 1000 OR h.revision<=0
   OR h.state NOT IN ('active','released','expired','consumed')
   OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(quote_lines) q
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

CREATE OR REPLACE FUNCTION refund_reservation_history_exact(wanted_order text) RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$
DECLARE owned checkout_orders%ROWTYPE; q jsonb; currency text; line jsonb; source jsonb; part jsonb;
 r refund_reservations%ROWTYPE; n integer:=0; i integer; count_lines integer;
 prior numeric[]; gross numeric; net numeric; vat numeric; captured numeric; captured_net numeric;
 captured_vat numeric; expected_vat numeric; sum_g numeric; sum_n numeric; sum_v numeric;
 source_g numeric:=0; source_n numeric:=0; source_v numeric:=0; last_at bigint:=0; sku text;
 seen_recognitions text[]:='{}'; original_attempt text; event_count integer;
 recognition payment_recognitions%ROWTYPE; greatest payment_recognitions%ROWTYPE;
 proved_through bigint; bounded_count integer:=0;
BEGIN
 SELECT * INTO owned FROM checkout_orders WHERE id=wanted_order;
 IF owned.id IS NULL OR octet_length(owned.snapshot::text)>65536 THEN RETURN false; END IF;
 q=owned.snapshot->'quote';
 currency=CASE owned.snapshot->'schema_version'
  WHEN '1'::jsonb THEN q->>'currency'
  WHEN '2'::jsonb THEN q->'original'->>'currency' END;
 IF jsonb_typeof(q->'lines') IS DISTINCT FROM 'array' OR currency IS DISTINCT FROM 'EUR'
  OR jsonb_array_length(q->'lines') NOT BETWEEN 1 AND 20 THEN RETURN false; END IF;
 count_lines=jsonb_array_length(q->'lines'); prior=array_fill(0::numeric,ARRAY[count_lines+1]);
 -- Verify the captured amounts independently and require distinct ordered SKU identities.
 FOR i IN 0..count_lines LOOP
  IF i=count_lines THEN source=q->'shipping';
  ELSE
   line=q->'lines'->i; source=line->'amounts'; sku=line->>'sku';
   IF sku IS NULL OR sku='' OR octet_length(sku)>128 OR EXISTS(
    SELECT 1 FROM jsonb_array_elements(q->'lines') WITH ORDINALITY x(v,j)
     WHERE j<=i AND v->>'sku'=sku) THEN RETURN false; END IF;
  END IF;
  captured=(source->>'gross_minor')::numeric; captured_net=(source->>'net_minor')::numeric; captured_vat=(source->>'vat_minor')::numeric;
  IF captured IS NULL OR captured_net IS NULL OR captured_vat IS NULL
   OR captured NOT BETWEEN 0 AND 9007199254740991 OR captured_net NOT BETWEEN 0 AND captured OR captured_vat NOT BETWEEN 0 AND captured
   OR source IS DISTINCT FROM jsonb_build_object('gross_minor',captured,'net_minor',captured_net,'vat_minor',captured_vat)
   OR trunc(captured)<>captured OR trunc(captured_net)<>captured_net OR trunc(captured_vat)<>captured_vat
   OR captured_net+captured_vat<>captured THEN RETURN false; END IF;
  source_g=source_g+captured; source_n=source_n+captured_net; source_v=source_v+captured_vat;
 END LOOP;
 IF source_g NOT BETWEEN 1 AND 9007199254740991 OR q->'total' IS DISTINCT FROM
  jsonb_build_object('gross_minor',source_g,'net_minor',source_n,'vat_minor',source_v) THEN RETURN false; END IF;
 -- Evidence is append-only; write guards separately claim the original attempt.
 -- Capture the bounded set first and prove its greatest referenced prefix.
 SELECT id INTO original_attempt FROM payment_attempts WHERE order_id=wanted_order;
 FOR r IN SELECT * FROM refund_reservations WHERE order_id=wanted_order ORDER BY generation LIMIT 17 LOOP
  bounded_count=bounded_count+1;
  IF bounded_count>16 OR original_attempt IS NULL
   OR r.attempt_id IS DISTINCT FROM original_attempt THEN RETURN false; END IF;
  SELECT * INTO recognition FROM payment_recognitions WHERE id=r.recognition_id;
  IF recognition.id IS NULL OR recognition.order_id IS DISTINCT FROM wanted_order
   OR recognition.attempt_id IS DISTINCT FROM original_attempt OR recognition.state IS DISTINCT FROM 'received'
   OR r.reserved_at<recognition.recorded_at OR r.reserved_at<owned.created_at
   OR r.reserved_at NOT BETWEEN 0 AND 9007199254740991 THEN RETURN false; END IF;
  IF greatest.id IS NULL OR recognition.through_observation>greatest.through_observation THEN
   greatest=recognition;
  END IF;
 END LOOP;
 IF bounded_count>0 THEN
  IF greatest.id IS NULL OR NOT payment_recognition_source(greatest,false) THEN RETURN false; END IF;
  proved_through=greatest.through_observation;
 END IF;
 FOR r IN SELECT * FROM refund_reservations WHERE order_id=wanted_order ORDER BY generation LIMIT 17 LOOP
  n=n+1;
  IF n>16 OR r.generation<>n OR r.reserved_at<last_at OR r.reserved_at>9007199254740991
   OR r.id !~ '^[a-f0-9]{32}$' OR r.event_id !~ '^[a-f0-9]{32}$'
   OR r.effect_key !~ '^[a-zA-Z0-9-]{1,128}$' OR r.reason='' OR btrim(r.reason)<>r.reason
   OR octet_length(r.reason)>240 OR r.reason ~ '[[:cntrl:]]' OR r.transaction_id IS NULL
   OR NOT EXISTS(SELECT 1 FROM staff_accounts WHERE id=r.actor_id)
   OR r.attempt_id IS DISTINCT FROM original_attempt OR octet_length(r.allocation::text)>16384
   OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=r.event_id AND e.aggregate_id=r.order_id
    AND e.kind='order.refund_reserved.v1' AND e.created_at=r.reserved_at
    AND e.payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,
     'reservation_id',r.id,'reservation_sha256',refund_reservation_digest(r)))
   OR jsonb_typeof(r.allocation->'lines') IS DISTINCT FROM 'array'
   OR jsonb_array_length(r.allocation->'lines')<>count_lines
   OR r.allocation IS DISTINCT FROM jsonb_build_object('schema_version',1,'lines',r.allocation->'lines',
     'shipping',r.allocation->'shipping','total',r.allocation->'total') THEN RETURN false; END IF;
  IF NOT r.recognition_id=ANY(seen_recognitions) THEN
   SELECT * INTO recognition FROM payment_recognitions WHERE id=r.recognition_id;
   IF recognition.id IS NULL OR recognition.order_id IS DISTINCT FROM wanted_order
    OR recognition.attempt_id IS DISTINCT FROM original_attempt OR recognition.state IS DISTINCT FROM 'received'
    OR r.reserved_at<recognition.recorded_at OR r.reserved_at<owned.created_at
    OR r.reserved_at NOT BETWEEN 0 AND 9007199254740991
    OR NOT payment_recognition_source_after_prefix(recognition,false,proved_through)
    OR NOT EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=owned.id
      AND e.kind='payment.recognized.v1' AND e.created_at=recognition.recorded_at
      AND e.payload=jsonb_build_object('schema_version',1,'order_id',owned.id,
       'attempt_id',recognition.attempt_id,'recognition_id',recognition.id,
       'recognition_sha256',payment_recognition_digest(recognition))) THEN RETURN false; END IF;
  END IF;
  seen_recognitions=array_append(seen_recognitions,r.recognition_id);
  sum_g=0;sum_n=0;sum_v=0;
  FOR i IN 0..count_lines LOOP
   IF i=count_lines THEN source=q->'shipping'; part=r.allocation->'shipping';
   ELSE source=q->'lines'->i->'amounts'; part=r.allocation->'lines'->i; END IF;
   gross=(part->>'gross_minor')::numeric; net=(part->>'net_minor')::numeric; vat=(part->>'vat_minor')::numeric;
   captured=(source->>'gross_minor')::numeric; captured_vat=(source->>'vat_minor')::numeric;
   IF gross IS NULL OR net IS NULL OR vat IS NULL OR gross NOT BETWEEN 0 AND 9007199254740991
    OR net NOT BETWEEN 0 AND gross OR vat NOT BETWEEN 0 AND gross
    OR trunc(gross)<>gross OR trunc(net)<>net OR trunc(vat)<>vat OR net+vat<>gross
    OR prior[i+1]+gross>captured THEN RETURN false; END IF;
   IF i=count_lines THEN
    IF part IS DISTINCT FROM jsonb_build_object('gross_minor',gross,'net_minor',net,'vat_minor',vat) THEN RETURN false; END IF;
   ELSE
    IF part IS DISTINCT FROM jsonb_build_object('sku',q->'lines'->i->>'sku','gross_minor',gross,'net_minor',net,'vat_minor',vat) THEN RETURN false; END IF;
   END IF;
   expected_vat=refund_reservation_vat_delta(captured,captured_vat,prior[i+1],gross);
   IF vat<>expected_vat THEN RETURN false; END IF;
   prior[i+1]=prior[i+1]+gross; sum_g=sum_g+gross;sum_n=sum_n+net;sum_v=sum_v+vat;
  END LOOP;
  IF sum_g NOT BETWEEN 1 AND 9007199254740991 OR r.allocation->'total' IS DISTINCT FROM
   jsonb_build_object('gross_minor',sum_g,'net_minor',sum_n,'vat_minor',sum_v) THEN RETURN false; END IF;
  last_at=r.reserved_at;
 END LOOP;
 IF n<>bounded_count THEN RETURN false; END IF;
 SELECT count(*) INTO event_count FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=wanted_order
  AND kind='order.refund_reserved.v1' LIMIT 17) bounded_events;
 IF event_count<>n THEN RETURN false; END IF;
 RETURN true;
EXCEPTION WHEN check_violation OR invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value OR datatype_mismatch OR division_by_zero THEN RETURN false;
END;
$$;
