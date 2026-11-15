use super::*;
use crate::{MIGRATOR, mollie::tests::order_stock_cases::fixture, order_stock::settle};
use commerce_domain::{late_stock::Outcome, order_shipment_components::ExpectedComponent};
use sqlx::Executor;
use std::sync::atomic::Ordering;

async fn staff(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES('planner',true),('other-planner',true); INSERT INTO staff_capabilities VALUES('planner','warehouse.manage'),('other-planner','stock.review')").await.expect("isolated capabilities");
    for (byte, actor) in [(71, "planner"), (72, "other-planner")] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,8000,1450)")
            .bind(SessionToken::from_bytes([byte;32]).digest().as_slice()).bind(actor)
            .execute(pool).await.expect("isolated staff session");
    }
}
fn item(sku: &str, units: u32) -> SkuUnits {
    SkuUnits {
        sku: sku.into(),
        units,
    }
}
fn token(byte: u8) -> SessionToken {
    SessionToken::from_bytes([byte; 32])
}
fn expected(code: &str, quantity: i64) -> ExpectedComponent {
    ExpectedComponent {
        code: code.into(),
        quantity,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn original_allocation_projects_one_bounded_batch_without_effects(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    staff(&pool).await;
    let order = &f.order.identity.id;
    let request = [item("SET-COVERS", 1), item("SET-FILLED", 1)];
    assert!(matches!(
        project(&pool, &token(71), f.time.as_ref(), order, &request).await,
        Err(PlanningError::Unavailable)
    ));
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("paid stock settled");
    let before = fixture::footprint(&pool).await;
    assert_eq!(
        project(&pool, &token(71), f.time.as_ref(), order, &request)
            .await
            .expect("authorized projection")
            .components,
        vec![expected("SEAT-COVER", 4), expected("SEAT-FOAM", 2)]
    );
    assert_eq!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            order,
            &[item("SET-COVERS", 1)]
        )
        .await
        .expect("covers only")
        .components,
        vec![expected("SEAT-COVER", 2)]
    );
    assert_eq!(before, fixture::footprint(&pool).await);
    assert!(matches!(
        project(&pool, &token(72), f.time.as_ref(), order, &request).await,
        Err(PlanningError::Staff(StaffError::Denied))
    ));
    for bad in [
        vec![],
        vec![item("SET-COVERS", 2)],
        vec![item("SET-COVERS", 0)],
        vec![item("UNKNOWN", 1)],
        vec![item("SET-COVERS", 1), item("SET-COVERS", 1)],
    ] {
        assert!(matches!(
            project(&pool, &token(71), f.time.as_ref(), order, &bad).await,
            Err(PlanningError::InvalidRequest)
        ));
    }
    fixture::observe(&pool, &f, "tr_SETTLEMENT", "open", "new-evidence").await;
    assert!(
        matches!(
            project(&pool, &token(71), f.time.as_ref(), order, &request).await,
            Err(PlanningError::Unavailable)
        ),
        "new evidence makes recognition stale"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_allocation_uses_the_original_frozen_component_demand(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    staff(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = late_stock::recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("late stock recovery")
        .outcome;
    assert!(matches!(outcome, Outcome::Late(_)));
    let before = fixture::footprint(&pool).await;
    let projection = project(
        &pool,
        &token(71),
        f.time.as_ref(),
        &f.order.identity.id,
        &[item("SET-FILLED", 1)],
    )
    .await
    .expect("late allocation projection");
    assert_eq!(
        projection.components,
        vec![expected("SEAT-COVER", 2), expected("SEAT-FOAM", 2)]
    );
    assert_eq!(before, fixture::footprint(&pool).await);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_expiry_during_real_attempt_claim_wait_rejects_projection(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    staff(&pool).await;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("settled source");
    let mut blocker = pool.begin().await.expect("attempt blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("claim held");
    let p = pool.clone();
    let time = f.time.clone();
    let order = f.order.identity.id.clone();
    let task = tokio::spawn(async move {
        project(
            &p,
            &token(71),
            time.as_ref(),
            &order,
            &[item("SET-COVERS", 1)],
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%'")
                .fetch_one(&pool).await.expect("actual attempt lock wait");
            if waiting > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("projection reached real attempt claim wait");
    f.time.0.store(2400, Ordering::SeqCst);
    blocker.commit().await.expect("release attempt claim");
    assert!(matches!(
        task.await.expect("projection task"),
        Err(PlanningError::Staff(StaffError::Denied))
    ));
    f.time.0.store(1500, Ordering::SeqCst);
    assert!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            &f.order.identity.id,
            &[item("SET-COVERS", 1)]
        )
        .await
        .is_ok(),
        "fresh authority positive control"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_allocation_receipt_is_not_treated_as_a_valid_projection(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    staff(&pool).await;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("settled source");
    let request = [item("SET-COVERS", 1)];
    assert!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            &f.order.identity.id,
            &request
        )
        .await
        .is_ok()
    );
    let mut tx = pool.begin().await.expect("isolated damage");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='order.stock_settled.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("remove receipt event only in test database");
    tx.commit().await.expect("publish damaged fixture");
    assert!(matches!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            &f.order.identity.id,
            &request
        )
        .await,
        Err(PlanningError::InvalidStoredData)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expired_authority_hides_unavailable_and_damaged_sources_after_attempt_wait(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    staff(&pool).await;
    let order = &f.order.identity.id;
    let request = [item("SET-COVERS", 1)];
    for damage in [false, true] {
        if damage {
            settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
                .await
                .expect("settled control");
            let mut tx = pool.begin().await.expect("isolated damage");
            tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='order.stock_settled.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("damage exact receipt");
            tx.commit().await.expect("commit test damage");
        }
        let fresh = project(&pool, &token(71), f.time.as_ref(), order, &request).await;
        assert!(
            matches!(fresh, Err(PlanningError::Unavailable)) && !damage
                || matches!(fresh, Err(PlanningError::InvalidStoredData)) && damage,
            "fresh authority sees the intended source outcome"
        );
        let mut blocker = pool.begin().await.expect("attempt blocker");
        sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(&f.frozen.snapshot().binding.id)
            .execute(&mut *blocker)
            .await
            .expect("claim held");
        let p = pool.clone();
        let time = f.time.clone();
        let owned = order.clone();
        let task = tokio::spawn(async move {
            project(
                &p,
                &token(71),
                time.as_ref(),
                &owned,
                &[item("SET-COVERS", 1)],
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_millis(800), async {
            loop {
                let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%'")
                    .fetch_one(&pool).await.expect("actual lock wait");
                if waiting > 0 { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("projection reached attempt claim");
        f.time.0.store(2400, Ordering::SeqCst);
        blocker.commit().await.expect("release attempt");
        assert!(
            matches!(
                task.await.expect("projection task"),
                Err(PlanningError::Staff(StaffError::Denied))
            ),
            "expired session cannot learn the source outcome"
        );
        f.time.0.store(1500, Ordering::SeqCst);
    }
}

#[test]
fn sealed_bom_loader_rejects_empty_and_oversized_source_counts() {
    for value in [-1, 0, 65, i64::MAX] {
        assert!(matches!(
            ensure_bom_count(value),
            Err(PlanningError::InvalidStoredData)
        ));
    }
    for value in [1, 64] {
        assert!(ensure_bom_count(value).is_ok());
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_receipt_and_invalid_identifiers_are_rejected(pool: PgPool) {
    let f = fixture::accepted(&pool, false, false).await;
    staff(&pool).await;
    let request = [item("SET-COVERS", 1)];
    assert!(matches!(
        project(&pool, &token(71), f.time.as_ref(), "bad", &request).await,
        Err(PlanningError::InvalidRequest)
    ));
    assert!(matches!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            &f.order.identity.id,
            &vec![item("SET-COVERS", 1); 21]
        )
        .await,
        Err(PlanningError::InvalidRequest)
    ));
    assert!(matches!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            &"f".repeat(32),
            &request
        )
        .await,
        Err(PlanningError::Unavailable)
    ));
    assert!(
        matches!(
            project(
                &pool,
                &token(71),
                f.time.as_ref(),
                &f.order.identity.id,
                &request
            )
            .await,
            Err(PlanningError::Unavailable)
        ),
        "an attempt without received recognition cannot project"
    );
}

#[test]
fn source_errors_remain_closed_and_do_not_create_projection() {
    assert!(matches!(
        settlement_error(SettlementError::NotFound),
        PlanningError::Unavailable
    ));
    assert!(matches!(
        settlement_error(SettlementError::InvalidStoredData),
        PlanningError::InvalidStoredData
    ));
    assert!(matches!(
        payment_error(PaymentStoreError::Unavailable),
        PlanningError::Unavailable
    ));
    assert!(matches!(
        payment_error(PaymentStoreError::InvalidStoredData),
        PlanningError::InvalidStoredData
    ));
    assert!(matches!(
        late_error(LateStockError::Unavailable),
        PlanningError::Unavailable
    ));
    assert!(matches!(
        late_error(LateStockError::InvalidStoredData),
        PlanningError::InvalidStoredData
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_genuine_stock_review_stop_cannot_produce_a_projection(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    staff(&pool).await;
    let hold: String = sqlx::query_scalar(
        "SELECT id FROM stock_holds WHERE basket_group_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(&f.order.identity.stock_group_id)
    .fetch_one(&pool)
    .await
    .expect("original hold");
    crate::holds::transition_hold(
        &pool,
        &hold,
        crate::holds::HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "synthetic-release",
        },
        1500,
    )
    .await
    .expect("explicit original release");
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = late_stock::recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("record review stop")
        .outcome;
    assert!(matches!(outcome, Outcome::NeedsReview(_)));
    assert!(matches!(
        project(
            &pool,
            &token(71),
            f.time.as_ref(),
            &f.order.identity.id,
            &[item("SET-COVERS", 1)]
        )
        .await,
        Err(PlanningError::Unavailable)
    ));
}
