//! Independent damaged-import controls; no current warehouse comparison on history.
use super::*;
async fn damage(pool: &PgPool, sql: &str) {
    let mut tx = pool.begin().await.expect("owned corrupt stock fixture");
    tx.execute("ALTER TABLE order_stock_settlements DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE stock_movements DISABLE TRIGGER USER; ALTER TABLE stock_holds DISABLE TRIGGER USER; ALTER TABLE stock_hold_lines DISABLE TRIGGER USER; ALTER TABLE catalogue_bom_lines DISABLE TRIGGER USER").await.expect("only isolated fixture guards");
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *tx)
        .await
        .expect("fixed damage/repair SQL");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("genuine FKchecks before restoring guards");
    tx.execute("ALTER TABLE order_stock_settlements ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE stock_movements ENABLE TRIGGER USER; ALTER TABLE stock_holds ENABLE TRIGGER USER; ALTER TABLE stock_hold_lines ENABLE TRIGGER USER; ALTER TABLE catalogue_bom_lines ENABLE TRIGGER USER").await.expect("restore original guards");
    tx.commit()
        .await
        .expect("public readers see committed controlled damage");
}
async fn backup(pool: &PgPool) {
    pool.execute("CREATE TABLE stock_fixture_moves AS TABLE stock_movements; CREATE TABLE stock_fixture_holds AS TABLE stock_holds; CREATE TABLE stock_fixture_lines AS TABLE stock_hold_lines; CREATE TABLE stock_fixture_bom AS TABLE catalogue_bom_lines; CREATE TABLE stock_fixture_balances AS TABLE stock_balances; CREATE TABLE stock_fixture_settlements AS TABLE order_stock_settlements; CREATE TABLE stock_fixture_events AS TABLE commerce_outbox").await.expect("exact owned repair sources");
}
const RESTORE: &str = "DELETE FROM stock_movements; INSERT INTO stock_movements OVERRIDING SYSTEM VALUE SELECT * FROM stock_fixture_moves; UPDATE stock_holds h SET state=b.state FROM stock_fixture_holds b WHERE h.id=b.id; DELETE FROM stock_hold_lines; INSERT INTO stock_hold_lines SELECT * FROM stock_fixture_lines; DELETE FROM catalogue_bom_lines; INSERT INTO catalogue_bom_lines SELECT * FROM stock_fixture_bom; UPDATE stock_balances b SET (on_hand,quarantine,reserved)=(SELECT f.on_hand,f.quarantine,f.reserved FROM stock_fixture_balances f WHERE f.component_code=b.component_code); DELETE FROM order_stock_settlements; INSERT INTO order_stock_settlements OVERRIDING SYSTEM VALUE SELECT * FROM stock_fixture_settlements; DELETE FROM commerce_outbox; INSERT INTO commerce_outbox SELECT * FROM stock_fixture_events";
const RESEAL: &str = "UPDATE commerce_outbox e SET created_at=s.settled_at,payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'settlement_id',s.id,'settlement_sha256',order_stock_digest(s)) FROM order_stock_settlements s WHERE e.kind='order.stock_settled.v1' AND e.aggregate_id=s.order_id";
#[sqlx::test(migrator = "MIGRATOR")]
async fn settlement_invalid_original_custody_cannot_be_downgraded_to_expired_recovery(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = footprint(&pool).await;
    backup(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(SettlementError::RecoveryRequired)
    );
    for sql in [
        "DELETE FROM stock_movements WHERE sequence=(SELECT min(sequence) FROM stock_movements WHERE action='reserve')",
        "DELETE FROM stock_hold_lines WHERE (hold_id,component_code)=(SELECT hold_id,component_code FROM stock_hold_lines ORDER BY hold_id,component_code LIMIT 1)",
        "DELETE FROM catalogue_bom_lines WHERE sku='SET-COVERS'",
        "UPDATE stock_balances SET reserved=0 WHERE component_code='SEAT-COVER'",
        "INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after,hold_id) SELECT 'hold:'||id||':consume-hold:SEAT-COVER','SEAT-COVER','consume-hold',2,'payment-settlement','sealed BOM hold transition',-2,0,-2,8,1,2,id FROM stock_holds WHERE sku='SET-COVERS'",
    ] {
        damage(&pool, sql).await;
        let damaged = footprint(&pool).await;
        assert_eq!(
            settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
                .await
                .err(),
            Some(SettlementError::InvalidStoredData),
            "corrupt original custody: {sql}"
        );
        assert_eq!(footprint(&pool).await, damaged);
        damage(&pool, RESTORE).await;
        assert_eq!(footprint(&pool).await, before);
        assert_eq!(
            settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
                .await
                .err(),
            Some(SettlementError::RecoveryRequired),
            "exact intact repair is a distinct recovery outcome"
        );
    }
    f.time.0.store(1500, Ordering::SeqCst);
    assert!(
        !settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("intact active source control")
            .replayed
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn settlement_history_revalidates_closed_member_allocation_original_recognition_movements_and_orphan_event(
    pool: PgPool,
) {
    let f = accepted(&pool, false, false).await;
    let attempt = &f.frozen.snapshot().binding.id;
    resolve(&pool, f.time.as_ref(), f.ids.as_ref(), attempt, "zero")
        .await
        .expect("genuine original unconfirmed resolution");
    let unconfirmed = recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("original unconfirmed receipt")
        .recognition;
    observe(&pool, &f, "tr_SETTLEMENT", "paid", "paid").await;
    recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
        .await
        .expect("genuine current received");
    let original = settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine complete settlement")
        .settlement;
    let before = footprint(&pool).await;
    backup(&pool).await;
    for sql in [
        "UPDATE order_stock_settlements SET footprint='{}'".to_owned(),
        "UPDATE order_stock_settlements SET footprint=footprint||'{\"unknown\":true}'::jsonb".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(footprint,'{members,0,components,0,quantity}','1')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(jsonb_set(footprint,'{members,0,components,0,quantity}','1'),'{members,1,components,0,quantity}','3')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(jsonb_set(footprint,'{components,0,before,on_hand}','11'),'{components,0,after,on_hand}','7')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(footprint,'{members,0}','[]')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(footprint,'{members,0,components,0}','[]')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(footprint,'{components,0}','[]')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(footprint,'{components,0,before}','[]')".to_owned(),
        "UPDATE order_stock_settlements SET footprint=jsonb_set(footprint,'{components,0,after}','[]')".to_owned(),
        format!("UPDATE order_stock_settlements SET recognition_id='{}'",unconfirmed.id),
        "UPDATE stock_movements SET actor='forged-actor' WHERE action='consume-hold' AND sequence=(SELECT min(sequence) FROM stock_movements WHERE action='consume-hold')".to_owned(),
        "UPDATE stock_movements SET reason='forged-reason' WHERE action='consume-hold'".to_owned(),
        "UPDATE stock_movements SET quantity=3,on_hand_delta=-3,reserved_delta=-3 WHERE sequence=(SELECT min(sequence) FROM stock_movements WHERE action='consume-hold')".to_owned(),
        "INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after,hold_id) SELECT 'hold:'||id||':release-hold:SEAT-COVER','SEAT-COVER','release-hold',2,'extra-terminal','sealed BOM hold transition',0,0,-2,8,1,2,id FROM stock_holds WHERE sku='SET-COVERS'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='order.stock_settled.v1'".to_owned(),
        "DELETE FROM commerce_outbox WHERE kind='payment.recognized.v1'".to_owned(),
        "UPDATE stock_movements SET on_hand_after=on_hand_after+1 WHERE sequence=(SELECT min(sequence) FROM stock_movements WHERE action='consume-hold')".to_owned(),
        "UPDATE stock_movements SET actor='forged-reserve-owner' WHERE action='reserve'".to_owned(),
        "DELETE FROM stock_movements WHERE sequence=(SELECT min(sequence) FROM stock_movements WHERE action='consume-hold')".to_owned(),
        "UPDATE stock_holds SET state='active' WHERE sku='SET-COVERS'".to_owned(),
    ]{
        damage(&pool,&format!("{sql}; {RESEAL}")).await;
        assert_eq!(read_settlement(&pool,&f.order.identity.id).await.err(),Some(SettlementError::InvalidStoredData),"historical exact footprint: {sql}");assert_eq!(settle(&pool,&super::super::observation_cases::BadPorts,&super::super::observation_cases::BadPorts,&f.order.identity.id).await.err(),Some(SettlementError::InvalidStoredData),"canonical replay: {sql}");
        damage(&pool,RESTORE).await;assert_eq!(footprint(&pool).await,before);assert!(read_settlement(&pool,&f.order.identity.id).await.expect("repaired exact historical receipt")==Some(original.clone()));let replay=settle(&pool,&super::super::observation_cases::BadPorts,&super::super::observation_cases::BadPorts,&f.order.identity.id).await.expect("healthy exact replay without ports");assert!(replay.replayed&&replay.settlement==original);
    }
    damage(&pool, "DELETE FROM order_stock_settlements").await;
    assert_eq!(
        read_settlement(&pool, &f.order.identity.id).await.err(),
        Some(SettlementError::InvalidStoredData),
        "orphan event never means no prior settlement"
    );
    assert_eq!(
        settle(
            &pool,
            &super::super::observation_cases::BadPorts,
            &super::super::observation_cases::BadPorts,
            &f.order.identity.id
        )
        .await
        .err(),
        Some(SettlementError::InvalidStoredData)
    );
    damage(&pool, RESTORE).await;
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("orphan fully repaired")
            == Some(original)
    );
}

#[path = "order_stock_oversize.rs"]
mod oversize;
