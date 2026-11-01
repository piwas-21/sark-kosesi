//! Direct real PostgreSQL source-manifest guards remain independent of outer proofs.
use super::*;
use crate::MIGRATOR;
use sqlx::{Executor, PgPool};
async fn begin_damage(pool: &PgPool) -> Transaction<'_, Postgres> {
    let mut tx = pool.begin().await.expect("source guard transaction");
    tx.execute("ALTER TABLE basket_hold_groups DISABLE TRIGGER USER; ALTER TABLE basket_hold_members DISABLE TRIGGER USER; ALTER TABLE stock_holds DISABLE TRIGGER USER; ALTER TABLE stock_hold_lines DISABLE TRIGGER USER; ALTER TABLE catalogue_boms DISABLE TRIGGER USER; ALTER TABLE catalogue_bom_lines DISABLE TRIGGER USER; ALTER TABLE physical_components DISABLE TRIGGER USER; ALTER TABLE stock_movements DISABLE TRIGGER USER").await.expect("only isolated fixture source guards");
    tx
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn original_manifest_guards_independently_reject_each_changed_group_hold_and_bom_binding(
    pool: PgPool,
) {
    let (_, _, _, _, order) = crate::payment_test_fixture::accepted(&pool).await;
    let mut healthy = pool.begin().await.expect("genuine original manifest");
    let source = members(&mut healthy, &order, true)
        .await
        .expect("complete originally accepted source");
    assert_eq!(source.members.len(), 1);
    assert_eq!(
        source.members[0]
            .components
            .iter()
            .map(|q| (q.component_code.as_str(), q.quantity))
            .collect::<Vec<_>>(),
        vec![("SEAT-COVER", 6), ("SEAT-FOAM", 6)]
    );
    healthy.rollback().await.expect("healthy proof rollback");
    for sql in [
        "UPDATE basket_hold_groups SET sealed=false",
        "UPDATE basket_hold_groups SET owner_id='ffffffffffffffffffffffffffffffff'",
        "UPDATE basket_hold_groups SET created_at=1099",
        "UPDATE basket_hold_groups SET expires_at=1999",
        "ALTER TABLE basket_hold_groups DROP CONSTRAINT basket_hold_groups_members_count_check; UPDATE basket_hold_groups SET members_count=0",
        "UPDATE stock_holds SET basket_group_id=NULL",
        "DELETE FROM basket_hold_members",
        "UPDATE basket_hold_members SET units=2",
        "UPDATE stock_holds SET owner_id='ffffffffffffffffffffffffffffffff'",
        "UPDATE stock_holds SET created_at=1099",
        "UPDATE stock_holds SET expires_at=1999",
        "ALTER TABLE stock_holds DROP CONSTRAINT stock_holds_sku_revision_fkey; ALTER TABLE basket_hold_members DROP CONSTRAINT basket_hold_members_sku_revision_fkey; UPDATE stock_holds SET revision=2; UPDATE basket_hold_members SET revision=2",
        "UPDATE stock_holds SET units=2; UPDATE basket_hold_members SET units=2",
        "UPDATE stock_holds SET state='building'",
        "ALTER TABLE basket_hold_members DROP CONSTRAINT basket_hold_members_hold_id_fkey; UPDATE basket_hold_members SET hold_id='ffffffffffffffffffffffffffffffff'",
        "UPDATE catalogue_boms SET sealed=false WHERE sku='SET-FILLED'",
        "DELETE FROM catalogue_bom_lines WHERE sku='SET-FILLED'",
        "DELETE FROM stock_hold_lines WHERE component_code='SEAT-FOAM'",
        "UPDATE stock_hold_lines SET quantity=5 WHERE component_code='SEAT-COVER'",
        "ALTER TABLE physical_components DROP CONSTRAINT physical_components_kind_check; UPDATE physical_components SET kind='other' WHERE code='SEAT-COVER'",
    ] {
        let mut tx = begin_damage(&pool).await;
        tx.execute(sql)
            .await
            .expect("controlled independently invalid source field");
        assert_eq!(
            members(&mut tx, &order, true).await.err(),
            Some(SettlementError::InvalidStoredData),
            "manifest source: {sql}"
        );
        tx.rollback().await.expect("exact source/schema restored");
        let mut repaired = pool.begin().await.expect("paired original source control");
        assert!(
            members(&mut repaired, &order, true)
                .await
                .expect("exact repaired original source")
                .members
                == source.members
        );
        repaired.rollback().await.expect("rollback");
    }
    for change in [0, 1] {
        let mut wrong = order.clone();
        if change == 0 {
            wrong.quote.lines.clear();
        } else {
            wrong.quote.lines[0].sku = "UNSELECTED".into();
        }
        let mut tx = pool.begin().await.expect("wrong selected manifest");
        assert_eq!(
            members(&mut tx, &wrong, true).await.err(),
            Some(SettlementError::InvalidStoredData)
        );
        tx.rollback().await.expect("rollback");
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn original_component_balance_guards_require_complete_valid_and_physically_backed_active_source(
    pool: PgPool,
) {
    let (_, _, _, _, order) = crate::payment_test_fixture::accepted(&pool).await;
    let mut tx = pool.begin().await.expect("original members");
    let source = members(&mut tx, &order, true)
        .await
        .expect("genuine active source");
    let original = balances(&mut tx, &source.members, &source.active_members)
        .await
        .expect("genuine backing");
    assert_eq!(original.len(), 2);
    assert!(original.iter().all(|c| c.quantity == 6
        && c.before.on_hand == 6
        && c.before.reserved == 6
        && c.after.on_hand == 0
        && c.after.reserved == 0));
    tx.rollback().await.expect("rollback");
    for sql in [
        "DELETE FROM stock_balances WHERE component_code='SEAT-COVER'",
        "ALTER TABLE stock_balances DROP CONSTRAINT stock_balances_check; UPDATE stock_balances SET on_hand=5 WHERE component_code='SEAT-COVER'",
        "UPDATE stock_balances SET reserved=5 WHERE component_code='SEAT-COVER'",
    ] {
        let mut tx = pool.begin().await.expect("physical invalid source");
        tx.execute(sql)
            .await
            .expect("isolated invalid physical backing");
        assert_eq!(
            balances(&mut tx, &source.members, &source.active_members)
                .await
                .err(),
            Some(SettlementError::InvalidStoredData)
        );
        tx.rollback().await.expect("restore physical source");
        let mut repaired = pool.begin().await.expect("paired original physical source");
        assert!(
            balances(&mut repaired, &source.members, &source.active_members)
                .await
                .expect("restored exact physical backing")
                == original
        );
        repaired.rollback().await.expect("rollback");
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn original_attempt_lookup_rejects_malformed_stored_identity_before_later_source_proofs(
    pool: PgPool,
) {
    let (store, _, session, csrf, order) = crate::payment_test_fixture::accepted(&pool).await;
    let owner = crate::guest_baskets::GuestCredential::from_session(&session).expect("owner");
    let receipt = crate::payment_attempts::prepare_attempt(
        &store,
        &owner,
        &csrf,
        "payment",
        &crate::payment_test_fixture::intent(&order),
        &crate::payment_test_fixture::config(),
    )
    .await
    .expect("genuine original prepared attempt");
    let mut tx = pool.begin().await.expect("healthy original identity");
    assert_eq!(
        attempt(&mut tx, &order.identity.id)
            .await
            .expect("original attempt"),
        receipt.frozen.snapshot().binding.id
    );
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("isolated malformed identity");
    tx.execute("ALTER TABLE payment_attempts DISABLE TRIGGER USER; ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_id_check; UPDATE payment_attempts SET id='broken',idempotency_key='piwas-test-payment-broken'").await.expect("malformed imported identity");
    assert_eq!(
        attempt(&mut tx, &order.identity.id).await.err(),
        Some(SettlementError::InvalidStoredData)
    );
    tx.rollback().await.expect("restore exact identity/schema");
}
