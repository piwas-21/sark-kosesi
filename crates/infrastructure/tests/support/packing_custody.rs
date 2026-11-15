use super::*;
use crate::{MIGRATOR, mollie::tests::order_stock_cases::fixture, order_stock::settle};
use sqlx::Executor;

async fn setup(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES('packer',true),('no-warehouse',true); INSERT INTO staff_capabilities VALUES('packer','warehouse.manage'),('no-warehouse','stock.review')").await.expect("staff");
    for (byte, actor) in [(71, "packer"), (72, "no-warehouse")] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,8000,1450)")
            .bind(SessionToken::from_bytes([byte;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("session");
    }
}
fn token(byte: u8) -> SessionToken {
    SessionToken::from_bytes([byte; 32])
}
fn units(sku: &str) -> Units {
    Units {
        sku: sku.into(),
        units: 1,
    }
}
fn part(code: &str, quantity: i64) -> Component {
    Component {
        code: code.into(),
        quantity,
    }
}
fn packed<'a>(
    order: &'a str,
    key: &'a str,
    items: &'a [Units],
    packages: &'a [Vec<Component>],
) -> Request<'a> {
    Request {
        order_id: order,
        effect_key: key,
        confirmed: true,
        action: Action::Packed {
            units: items,
            packages,
        },
    }
}
fn released<'a>(order: &'a str, key: &'a str, target: &'a str) -> Request<'a> {
    Request {
        order_id: order,
        effect_key: key,
        confirmed: true,
        action: Action::Released { target_id: target },
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn packing_inspection_reports_assertions_and_historical_release_without_writes(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), order).await,
        Err(Error::Unavailable)
    ));
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let empty = inspect(&pool, &token(71), f.time.as_ref(), order)
        .await
        .expect("empty history");
    assert!(empty.history.is_empty());
    assert_eq!(
        empty
            .lines
            .iter()
            .map(|line| (
                line.sku.as_str(),
                line.ordered,
                line.active_asserted,
                line.not_asserted
            ))
            .collect::<Vec<_>>(),
        vec![("SET-COVERS", 1, 0, 1), ("SET-FILLED", 1, 0, 1)]
    );
    assert!(matches!(
        inspect(&pool, &token(72), f.time.as_ref(), order).await,
        Err(Error::Denied)
    ));
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let first = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "inspect-pack", &items, &packages),
    )
    .await
    .expect("pack");
    let active = inspect(&pool, &token(71), f.time.as_ref(), order)
        .await
        .expect("active");
    assert_eq!(
        active
            .lines
            .iter()
            .map(|line| (line.sku.as_str(), line.active_asserted, line.not_asserted))
            .collect::<Vec<_>>(),
        vec![("SET-COVERS", 0, 1), ("SET-FILLED", 1, 0)]
    );
    assert_eq!(active.history.len(), 1);
    assert_eq!(active.history[0].id, first.record.id);
    assert_eq!(active.history[0].action, "packed");
    fixture::observe(&pool, &f, "tr_SETTLEMENT", "open", "inspect-new-evidence").await;
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "inspect-release", &first.record.id),
    )
    .await
    .expect("release after stale recognition");
    let footprint = fixture::footprint(&pool).await;
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM packing_custody), (SELECT count(*) FROM commerce_outbox)",
    )
    .fetch_one(&pool)
    .await
    .expect("counts");
    let history = inspect(&pool, &token(71), f.time.as_ref(), order)
        .await
        .expect("historical inspection");
    assert_eq!(
        history
            .lines
            .iter()
            .map(|line| line.active_asserted)
            .collect::<Vec<_>>(),
        vec![0, 0]
    );
    assert_eq!(history.history.len(), 2);
    assert_eq!(history.history[1].action, "released");
    assert_eq!(
        history.history[1].target_id.as_deref(),
        Some(first.record.id.as_str())
    );
    assert_eq!(
        before,
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM packing_custody), (SELECT count(*) FROM commerce_outbox)"
        )
        .fetch_one(&pool)
        .await
        .expect("unchanged counts")
    );
    assert_eq!(footprint, fixture::footprint(&pool).await);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn packing_inspection_fails_closed_on_damaged_event(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "inspect-damage", &items, &packages),
    )
    .await
    .expect("pack");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox SET payload='{}' WHERE kind='order.packing_custody_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("isolated damage");
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), order).await,
        Err(Error::InvalidStoredData)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn packing_inspection_masks_outcome_when_mfa_expires_during_attempt_wait(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("settlement");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("lock attempt");
    let (p, clock, order) = (pool.clone(), f.time.clone(), f.order.identity.id.clone());
    let task = tokio::spawn(async move { inspect(&p, &token(71), clock.as_ref(), &order).await });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%'").fetch_one(&pool).await.expect("waiter");
            if waiting > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("attempt waiter");
    f.time.0.store(2351, Ordering::SeqCst);
    blocker.commit().await.expect("unlock");
    assert!(matches!(task.await.expect("task"), Err(Error::Denied)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn packing_inspection_rejects_invalid_and_unavailable_empty_sources(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), "invalid").await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), &"f".repeat(32)).await,
        Err(Error::Unavailable)
    ));
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    assert!(
        inspect(&pool, &token(71), f.time.as_ref(), order)
            .await
            .expect("current source")
            .history
            .is_empty()
    );
    fixture::observe(&pool, &f, "tr_SETTLEMENT", "open", "inspect-empty-stale").await;
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), order).await,
        Err(Error::Unavailable)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("unchanged journal");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn packing_inspection_expired_wait_masks_damaged_history(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "inspect-expired-damage", &items, &packages),
    )
    .await
    .expect("pack");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox SET payload='{}' WHERE kind='order.packing_custody_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("isolated damage");
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), order).await,
        Err(Error::InvalidStoredData)
    ));
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("lock attempt");
    let (p, clock, order) = (pool.clone(), f.time.clone(), order.clone());
    let task = tokio::spawn(async move { inspect(&p, &token(71), clock.as_ref(), &order).await });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%'").fetch_one(&pool).await.expect("waiter");
            if waiting > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("attempt waiter");
    f.time.0.store(2351, Ordering::SeqCst);
    blocker.commit().await.expect("unlock");
    assert!(matches!(task.await.expect("task"), Err(Error::Denied)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("unchanged journal");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn custody_records_packages_replays_and_releases_without_stock_effects(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let before = fixture::footprint(&pool).await;
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2)], vec![part("SEAT-FOAM", 2)]];
    let result = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "pack-one", &items, &packages),
    )
    .await
    .expect("packed");
    assert!(!result.replayed);
    let again = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "pack-one", &items, &packages),
    )
    .await
    .expect("replay");
    assert!(again.replayed);
    assert_eq!(result.record, again.record);
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "pack-two", &items, &packages)
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(72),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "bad-role", &items, &packages)
        )
        .await,
        Err(Error::Denied)
    ));
    let changed = [vec![part("SEAT-COVER", 1)], vec![part("SEAT-FOAM", 2)]];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "pack-one", &items, &changed)
        )
        .await,
        Err(Error::Conflict)
    ));
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "release-one", &result.record.id),
    )
    .await
    .expect("release");
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "release-twice", &result.record.id)
        )
        .await,
        Err(Error::Conflict)
    ));
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "pack-after-release", &items, &packages),
    )
    .await
    .expect("pack after release");
    assert_eq!(before, fixture::footprint(&pool).await);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_packs_do_not_double_claim_units(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = f.order.identity.id.clone();
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &order)
        .await
        .expect("settlement");
    let mut tasks = Vec::new();
    for key in ["race-one", "race-two"] {
        let (p, t, ids, o) = (pool.clone(), f.time.clone(), f.ids.clone(), order.clone());
        tasks.push(tokio::spawn(async move {
            let items = [units("SET-FILLED")];
            let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
            record(
                &p,
                &token(71),
                t.as_ref(),
                ids.as_ref(),
                packed(&o, key, &items, &packages),
            )
            .await
        }));
    }
    let one = tasks.remove(0).await.expect("task");
    let two = tasks.remove(0).await.expect("task");
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(matches!(
        (&one, &two),
        (Ok(_), Err(Error::InvalidRequest)) | (Err(Error::InvalidRequest), Ok(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody WHERE order_id=$1")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_payment_blocks_new_pack_but_does_not_prevent_record_release(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let saved = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "first-pack", &items, &packages),
    )
    .await
    .expect("packed");
    fixture::observe(&pool, &f, "tr_SETTLEMENT", "open", "new-evidence").await;
    let covers = [units("SET-COVERS")];
    let cover_packages = [vec![part("SEAT-COVER", 2)]];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "stale-pack", &covers, &cover_packages)
        )
        .await,
        Err(Error::Unavailable)
    ));
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "release-stale", &saved.record.id),
    )
    .await
    .expect("release historical assertion");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "first-pack", &items, &packages)
        )
        .await
        .expect("historical replay")
        .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_event_rejects_history_until_exact_repair(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "intact", &items, &packages),
    )
    .await
    .expect("intact record");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox SET payload='{}' WHERE kind='order.packing_custody_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("isolated corruption");
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "intact", &items, &packages)
        )
        .await,
        Err(Error::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox e SET payload=packing_custody_event(p) FROM packing_custody p WHERE p.event_id=e.id AND e.kind='order.packing_custody_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("exact repair");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "intact", &items, &packages)
        )
        .await
        .expect("repaired replay")
        .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mfa_expiry_while_attempt_is_locked_rolls_back(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = f.order.identity.id.clone();
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &order)
        .await
        .expect("settlement");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("attempt lock");
    let (p, t, ids) = (pool.clone(), f.time.clone(), f.ids.clone());
    let task = tokio::spawn(async move {
        let items = [units("SET-FILLED")];
        let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
        record(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            packed(&order, "expire-wait", &items, &packages),
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
    }).await.expect("packing reached attempt claim wait");
    f.time.0.store(2351, Ordering::SeqCst);
    blocker.commit().await.expect("unlock");
    assert!(matches!(task.await.expect("task"), Err(Error::Denied)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_allocation_accepts_only_exact_shared_components_and_confirmation(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let order = &f.order.identity.id;
    crate::late_stock::recover(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("late allocation");
    let items = [units("SET-FILLED"), units("SET-COVERS")];
    let bad = [vec![part("SEAT-COVER", 3), part("SEAT-FOAM", 2)]];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "bad-totals", &items, &bad)
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    let correct = [
        vec![part("SEAT-COVER", 2)],
        vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)],
    ];
    let mut not_confirmed = packed(order, "no-confirmation", &items, &correct);
    not_confirmed.confirmed = false;
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            not_confirmed
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    let result = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "late-pack", &items, &correct),
    )
    .await
    .expect("exact shared totals");
    assert_eq!(result.record.allocation_kind, "late");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn completed_journal_seals_payment_source_until_commit(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let first = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "fence-pack", &items, &packages),
    )
    .await
    .expect("pack")
    .record;
    let mut release = first.clone();
    release.id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    release.event_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    release.effect_key = "fence-release".into();
    release.generation = 2;
    release.action = "released".into();
    release.target_id = Some(first.id);
    release.units = sqlx::types::Json(Vec::new());
    release.packages = sqlx::types::Json(Vec::new());
    let mut tx = pool.begin().await.expect("isolated transaction");
    store::insert(&mut tx, &release)
        .await
        .expect("immediate constraint completes");
    let error = sqlx::query(
        "UPDATE payment_recognition_current SET recognition_id=recognition_id WHERE order_id=$1",
    )
    .bind(order)
    .execute(&mut *tx)
    .await
    .expect_err("seal rejects source change");
    assert!(
        error
            .to_string()
            .contains("Completed packing source cannot change")
    );
    tx.rollback().await.expect("rollback");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(
                order,
                "after-fence",
                release.target_id.as_deref().expect("target")
            )
        )
        .await
        .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn journal_capacity_reserves_a_release_for_each_active_pack(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    for i in 0..31 {
        let pack = record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, &format!("capacity-pack-{i}"), &items, &packages),
        )
        .await
        .expect("pack");
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, &format!("capacity-release-{i}"), &pack.record.id),
        )
        .await
        .expect("release");
    }
    let active = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "capacity-last-pack", &items, &packages),
    )
    .await
    .expect("last admissible pack");
    let other = [units("SET-COVERS")];
    let other_packages = [vec![part("SEAT-COVER", 2)]];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "capacity-exhausted", &other, &other_packages)
        )
        .await,
        Err(Error::Conflict)
    ));
    let mut forged = active.record.clone();
    forged.id = "cccccccccccccccccccccccccccccccc".into();
    forged.event_id = "dddddddddddddddddddddddddddddddd".into();
    forged.effect_key = "capacity-native-control".into();
    forged.generation = 64;
    forged.units = sqlx::types::Json(other.to_vec());
    forged.packages = sqlx::types::Json(other_packages.to_vec());
    let mut tx = pool.begin().await.expect("native capacity transaction");
    assert!(
        store::insert(&mut tx, &forged).await.is_err(),
        "native invariant rejects a direct over-capacity pack"
    );
    tx.rollback().await.expect("rollback direct insert");
    let release = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "capacity-final-release", &active.record.id),
    )
    .await
    .expect("reserved release slot");
    assert_eq!(release.record.generation, 64);
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "capacity-full", &other, &other_packages)
        )
        .await,
        Err(Error::Conflict)
    ));
    let exact: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("native invariant");
    assert!(exact);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_history_rejects_resealed_null_package_quantity(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "native-control", &items, &packages),
    )
    .await
    .expect("pack");
    pool.execute("CREATE TABLE packing_fixture AS TABLE packing_custody; ALTER TABLE packing_custody DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE packing_custody SET packages=jsonb_set(packages,'{0,0,quantity}','null'); UPDATE commerce_outbox e SET payload=packing_custody_event(p) FROM packing_custody p WHERE e.id=p.event_id; ALTER TABLE packing_custody ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("isolated damage and reseal");
    let valid: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("native corrupted check");
    assert!(!valid);
    pool.execute("ALTER TABLE packing_custody DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE packing_custody p SET packages=f.packages FROM packing_fixture f WHERE p.id=f.id; UPDATE commerce_outbox e SET payload=packing_custody_event(p) FROM packing_custody p WHERE e.id=p.event_id; ALTER TABLE packing_custody ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("exact repair and reseal");
    let valid: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("native repaired check");
    assert!(valid);
}

async fn allocation_chronology_boundary(pool: &PgPool, late: bool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(pool, false, true).await;
    setup(pool).await;
    let order = &f.order.identity.id;
    let allocated_at: i64 = if late { 2000 } else { 1600 };
    f.time.0.store(
        u64::try_from(allocated_at).expect("nonnegative time"),
        Ordering::SeqCst,
    );
    if late {
        crate::late_stock::recover(pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .expect("late allocation");
    } else {
        settle(pool, f.time.as_ref(), f.ids.as_ref(), order)
            .await
            .expect("original settlement");
    }
    let prior_time = allocated_at - 1;
    f.time.0.store(
        u64::try_from(prior_time).expect("nonnegative time"),
        Ordering::SeqCst,
    );
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    assert!(
        matches!(
            record(
                pool,
                &token(71),
                f.time.as_ref(),
                f.ids.as_ref(),
                packed(order, "before-allocation", &items, &packages)
            )
            .await,
            Err(Error::ClockUnavailable)
        ),
        "staff time before allocation must not create a record"
    );
    let mut tx = pool.begin().await.expect("isolated native transaction");
    let attempt = crate::order_stock::source::attempt(&mut tx, order)
        .await
        .expect("attempt");
    crate::order_stock::source::claim_attempt(&mut tx, &attempt)
        .await
        .expect("claim");
    let source = shipment_planning::source(&mut tx, order, &attempt, None)
        .await
        .expect("allocation source");
    assert_eq!(source.allocation_at, allocated_at);
    let forged = Record {
        id: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
        order_id: order.clone(),
        attempt_id: attempt,
        recognition_id: source.recognition_id,
        allocation_kind: source.allocation_kind.into(),
        allocation_id: source.allocation_id,
        actor_id: "packer".into(),
        effect_key: "native-before-allocation".into(),
        generation: 1,
        action: "packed".into(),
        target_id: None,
        units: sqlx::types::Json(items.to_vec()),
        packages: sqlx::types::Json(packages.to_vec()),
        recorded_at: prior_time,
        event_id: "ffffffffffffffffffffffffffffffff".into(),
    };
    assert!(
        store::insert(&mut tx, &forged).await.is_err(),
        "native custody rejects a pre-allocation record"
    );
    tx.rollback().await.expect("rollback");
    f.time.0.store(
        u64::try_from(allocated_at).expect("nonnegative time"),
        Ordering::SeqCst,
    );
    let saved = record(
        pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "after-allocation", &items, &packages),
    )
    .await
    .expect("boundary at allocation succeeds");
    let valid: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(pool)
        .await
        .expect("valid native history");
    assert!(valid);
    pool.execute("ALTER TABLE packing_custody DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER").await.expect("test-only guard disable");
    sqlx::query("UPDATE packing_custody SET recorded_at=$1 WHERE id=$2")
        .bind(prior_time)
        .bind(&saved.record.id)
        .execute(pool)
        .await
        .expect("time damage");
    pool.execute("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=packing_custody_event(p) FROM packing_custody p WHERE e.id=p.event_id; ALTER TABLE packing_custody ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("resealed historical damage");
    let valid: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(pool)
        .await
        .expect("damaged native history");
    assert!(
        !valid,
        "resealing a pre-allocation history cannot authorize it"
    );
    assert!(matches!(
        record(
            pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "after-allocation", &items, &packages)
        )
        .await,
        Err(Error::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE packing_custody DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER").await.expect("test-only guard disable");
    sqlx::query("UPDATE packing_custody SET recorded_at=$1 WHERE id=$2")
        .bind(allocated_at)
        .bind(&saved.record.id)
        .execute(pool)
        .await
        .expect("exact time repair");
    pool.execute("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=packing_custody_event(p) FROM packing_custody p WHERE e.id=p.event_id; ALTER TABLE packing_custody ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("exact event repair");
    assert!(
        record(
            pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "after-allocation", &items, &packages)
        )
        .await
        .expect("repaired replay")
        .replayed
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn original_settlement_precedes_every_packing_record(pool: PgPool) {
    allocation_chronology_boundary(&pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_allocation_precedes_every_packing_record(pool: PgPool) {
    allocation_chronology_boundary(&pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn history_reconstruction_rejects_changed_identity_contents_and_transitions(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let filled = [units("SET-FILLED")];
    let filled_packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let covers = [units("SET-COVERS")];
    let cover_packages = [vec![part("SEAT-COVER", 2)]];
    let first = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "history-one", &filled, &filled_packages),
    )
    .await
    .expect("first")
    .record;
    let second = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "history-two", &covers, &cover_packages),
    )
    .await
    .expect("second")
    .record;
    let release = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "history-release", &first.id),
    )
    .await
    .expect("release")
    .record;
    let mut tx = pool.begin().await.expect("source transaction");
    let source = shipment_planning::source(&mut tx, order, &first.attempt_id, None)
        .await
        .expect("exact source");
    tx.rollback().await.expect("rollback source read");
    let records = vec![first.clone(), second.clone(), release.clone()];
    let (prior, count) = model::validate(&source, &records).expect("complete history");
    assert_eq!(count, 1);
    assert!(prior.iter().any(|x| x.sku == "SET-COVERS" && x.units == 1));
    assert!(prior.iter().any(|x| x.sku == "SET-FILLED" && x.units == 0));
    let mut cases = Vec::new();
    let mut r = first.clone();
    r.generation = 2;
    cases.push(vec![r]);
    let mut r = first.clone();
    r.order_id = "different".into();
    cases.push(vec![r]);
    let mut r = first.clone();
    r.allocation_kind = "other".into();
    cases.push(vec![r]);
    let mut r = first.clone();
    r.allocation_id = "different".into();
    cases.push(vec![r]);
    let mut r = first.clone();
    r.recorded_at = source.allocation_at - 1;
    cases.push(vec![r]);
    let mut r = first.clone();
    r.action = "unknown".into();
    cases.push(vec![r]);
    let mut r = first.clone();
    r.units = sqlx::types::Json(Vec::new());
    cases.push(vec![r]);
    let mut r = first.clone();
    r.packages = sqlx::types::Json(Vec::new());
    cases.push(vec![r]);
    let mut r = first.clone();
    r.packages.0[0][0].quantity = 1;
    cases.push(vec![r]);
    let mut r = first.clone();
    r.target_id = Some("unexpected".into());
    cases.push(vec![r]);
    let mut r = second.clone();
    r.id = first.id.clone();
    cases.push(vec![first.clone(), r]);
    let mut r = release.clone();
    r.target_id = None;
    cases.push(vec![first.clone(), second.clone(), r]);
    let mut r = release.clone();
    r.target_id = Some("unrelated".into());
    cases.push(vec![first.clone(), second.clone(), r]);
    let mut r = release.clone();
    r.units = first.units.clone();
    cases.push(vec![first.clone(), second.clone(), r]);
    let mut r = release.clone();
    r.packages = first.packages.clone();
    cases.push(vec![first.clone(), second.clone(), r]);
    let mut earlier = first.clone();
    earlier.recorded_at += 1;
    let mut later = second.clone();
    later.recorded_at = earlier.recorded_at;
    let mut r = release.clone();
    r.recorded_at = source.allocation_at;
    cases.push(vec![earlier, later, r]);
    for invalid in cases {
        assert!(matches!(
            model::validate(&source, &invalid),
            Err(Error::InvalidStoredData)
        ));
    }
    let mut long_history = Vec::new();
    for i in 0..31 {
        let mut p = first.clone();
        p.id = format!("{i:032x}");
        p.generation = (i * 2 + 1) as i16;
        let mut r = release.clone();
        r.target_id = Some(p.id.clone());
        r.generation = (i * 2 + 2) as i16;
        long_history.push(p);
        long_history.push(r);
    }
    let mut p = first.clone();
    p.id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    p.generation = 63;
    long_history.push(p);
    let mut p = second;
    p.id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    p.generation = 64;
    long_history.push(p);
    assert!(
        matches!(
            model::validate(&source, &long_history),
            Err(Error::InvalidStoredData)
        ),
        "a stored prefix must reserve release slots"
    );
}

#[test]
fn package_boundaries_reject_bad_identifiers_counts_and_duplicates() {
    for packages in [
        vec![],
        vec![vec![]],
        vec![vec![part("", 1)]],
        vec![vec![part("SEAT-COVER", 0)]],
        vec![vec![part("SEAT-COVER", 1_000_000_001)]],
        vec![vec![part("SEAT-COVER", 1), part("SEAT-COVER", 1)]],
        vec![vec![part("SEAT-COVER", 1)]; 33],
        vec![vec![part("SEAT-COVER", 1); 65]],
    ] {
        assert!(matches!(
            model::check_packages(&packages),
            Err(Error::InvalidRequest)
        ));
    }
    let totals = model::check_packages(&[
        vec![part("SEAT-COVER", 2)],
        vec![part("SEAT-COVER", 3), part("SEAT-FOAM", 2)],
    ])
    .expect("distinct package contents");
    assert_eq!(totals.get("SEAT-COVER"), Some(&5));
    assert_eq!(totals.get("SEAT-FOAM"), Some(&2));
}

struct NoIds;
impl Identifiers for NoIds {
    fn next(&self) -> Result<[u8; 16], commerce_application::request_context::ContextError> {
        Err(commerce_application::request_context::ContextError::EntropyUnavailable)
    }
}
struct NoClock;
impl Clock for NoClock {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        Err(commerce_application::request_context::ContextError::ClockUnavailable)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn input_identity_clock_and_effect_conflicts_fail_without_writes(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    pool.execute("INSERT INTO staff_accounts VALUES('other-packer',true); INSERT INTO staff_capabilities VALUES('other-packer','warehouse.manage')").await.expect("second authorized actor");
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'other-packer',1000,8000,1450)").bind(token(73).digest().as_slice()).execute(&pool).await.expect("second session");
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settled");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    for request in [
        packed("invalid", "bad-order", &items, &packages),
        packed(order, "bad key", &items, &packages),
        packed(order, "empty-units", &[], &packages),
        released(order, "bad-target", "x"),
    ] {
        assert!(matches!(
            record(&pool, &token(71), f.time.as_ref(), f.ids.as_ref(), request).await,
            Err(Error::InvalidRequest)
        ));
    }
    let oversized = vec![units("SET-FILLED"); 21];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "oversized", &oversized, &packages)
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            &NoClock,
            f.ids.as_ref(),
            packed(order, "bad-clock", &items, &packages)
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            &NoIds,
            packed(order, "bad-entropy", &items, &packages)
        )
        .await,
        Err(Error::EntropyUnavailable)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "missing-target", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "missing-order",
                &items,
                &packages
            )
        )
        .await,
        Err(Error::Unavailable)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("no side effects");
    assert_eq!(count, 0);
    f.time.0.store(1700, Ordering::SeqCst);
    let first = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "actor-effect", &items, &packages),
    )
    .await
    .expect("pack")
    .record;
    assert!(matches!(
        record(
            &pool,
            &token(73),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "actor-effect", &items, &packages)
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "actor-effect",
                &items,
                &packages
            )
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "actor-effect", &first.id)
        )
        .await,
        Err(Error::Conflict)
    ));
    f.time.0.store(1600, Ordering::SeqCst);
    let covers = [units("SET-COVERS")];
    let cover_packages = [vec![part("SEAT-COVER", 2)]];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "rewound-clock", &covers, &cover_packages)
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    f.time.0.store(1700, Ordering::SeqCst);
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "release-effect", &first.id),
    )
    .await
    .expect("release");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "release-effect", &first.id)
        )
        .await
        .expect("release replay")
        .replayed
    );
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "release-effect", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "release-effect", &items, &packages)
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "actor-effect", &covers, &cover_packages)
        )
        .await,
        Err(Error::Conflict)
    ));
}

#[test]
fn error_classification_preserves_closed_categories() {
    assert_eq!(staff(StaffError::InvalidRequest), Error::Denied);
    assert_eq!(staff(StaffError::ClockUnavailable), Error::ClockUnavailable);
    assert_eq!(
        staff(StaffError::Database(sqlx::Error::RowNotFound)),
        Error::Database
    );
    assert_eq!(source(PlanningError::Unavailable), Error::Unavailable);
    assert_eq!(
        source(PlanningError::Database(sqlx::Error::RowNotFound)),
        Error::Database
    );
    assert_eq!(
        source(PlanningError::InvalidStoredData),
        Error::InvalidStoredData
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_writes_and_oversize_imports_fail_closed(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settled");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let first = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "write-control", &items, &packages),
    )
    .await
    .expect("pack")
    .record;
    let mut next = first.clone();
    next.id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    next.event_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    next.effect_key = "suppressed-write".into();
    next.generation = 2;
    next.action = "released".into();
    next.target_id = Some(first.id.clone());
    next.units = sqlx::types::Json(Vec::new());
    next.packages = sqlx::types::Json(Vec::new());
    for table in ["packing_custody", "commerce_outbox"] {
        let mut tx = pool.begin().await.expect("fault transaction");
        tx.execute("CREATE FUNCTION packing_test_suppress() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").await.expect("fault function");
        let query = format!(
            "CREATE TRIGGER zz_packing_test_suppress BEFORE INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION packing_test_suppress()"
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(query))
            .execute(&mut *tx)
            .await
            .expect("isolated suppression");
        assert!(
            matches!(
                store::insert(&mut tx, &next).await,
                Err(Error::InvalidStoredData)
            ),
            "suppressed {table} cannot commit"
        );
        tx.rollback().await.expect("rollback suppression");
    }
    let mut tx = pool.begin().await.expect("oversized import transaction");
    tx.execute("ALTER TABLE packing_custody DROP CONSTRAINT packing_custody_generation_check; ALTER TABLE packing_custody DISABLE TRIGGER USER; CREATE OR REPLACE FUNCTION packing_custody_history_exact(wanted text) RETURNS boolean LANGUAGE sql AS $$ SELECT true $$").await.expect("isolate independent reader bound");
    sqlx::query("INSERT INTO packing_custody(id,order_id,attempt_id,recognition_id,allocation_kind,allocation_id,actor_id,effect_key,generation,action,target_id,units,packages,recorded_at,event_id,transaction_id) SELECT lpad(to_hex(n+70000),32,'0'),p.order_id,p.attempt_id,p.recognition_id,p.allocation_kind,p.allocation_id,p.actor_id,'oversize-'||n,n::smallint,'packed',NULL,p.units,p.packages,p.recorded_at,lpad(to_hex(n+90000),32,'0'),pg_current_xact_id() FROM packing_custody p CROSS JOIN generate_series(2,65) n WHERE p.id=$1")
        .bind(&first.id).execute(&mut *tx).await.expect("isolated oversized import");
    assert!(matches!(
        store::history(&mut tx, order).await,
        Err(Error::InvalidStoredData)
    ));
    tx.rollback().await.expect("rollback oversized import");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "write-positive", &first.id)
        )
        .await
        .is_ok(),
        "restored guards and valid write"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mfa_expiry_during_outbox_wait_rolls_back_record_and_event(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = f.order.identity.id.clone();
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &order)
        .await
        .expect("settlement");
    let mut blocker = pool.begin().await.expect("outbox blocker");
    blocker
        .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
        .await
        .expect("hold actual outbox write");
    let (p, t, ids, o) = (pool.clone(), f.time.clone(), f.ids.clone(), order.clone());
    let task = tokio::spawn(async move {
        let items = [units("SET-FILLED")];
        let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
        record(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            packed(&o, "outbox-expiry", &items, &packages),
        )
        .await
    });
    crate::payment_test_fixture::waiting(&pool).await;
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("uncommitted row invisible");
    assert_eq!(visible, 0);
    f.time.0.store(2351, Ordering::SeqCst);
    blocker.commit().await.expect("release outbox");
    assert!(matches!(task.await.expect("task"), Err(Error::Denied)));
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM packing_custody),(SELECT count(*) FROM commerce_outbox WHERE kind='order.packing_custody_recorded.v1')").fetch_one(&pool).await.expect("rollback effects");
    assert_eq!(counts, (0, 0));
    f.time.0.store(1500, Ordering::SeqCst);
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(&order, "outbox-expiry", &items, &packages)
        )
        .await
        .is_ok(),
        "fresh retry succeeds"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_actor_during_insert_fails_readback_and_rolls_back(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    pool.execute("INSERT INTO staff_accounts VALUES('injected-actor',true)")
        .await
        .expect("alternate stored identity");
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    pool.execute("CREATE FUNCTION packing_test_alter_actor() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.actor_id='injected-actor'; RETURN NEW; END $$; CREATE TRIGGER zz_test_alter_actor BEFORE INSERT ON packing_custody FOR EACH ROW EXECUTE FUNCTION packing_test_alter_actor()").await.expect("isolated write alteration");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "altered-actor", &items, &packages)
        )
        .await,
        Err(Error::InvalidStoredData)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("rollback");
    assert_eq!(count, 0);
    pool.execute("DROP TRIGGER zz_test_alter_actor ON packing_custody; DROP FUNCTION packing_test_alter_actor()").await.expect("restore original trigger set");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(order, "altered-actor", &items, &packages)
        )
        .await
        .is_ok(),
        "unmodified positive control"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn effect_key_conflict_after_advisory_wait_observes_authority_expiry(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = f.order.identity.id.clone();
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(&order, "shared-effect", &items, &packages),
    )
    .await
    .expect("owned effect");
    for expired in [false, true] {
        f.time.0.store(1500, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("advisory blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind("packing-custody:shared-effect")
            .execute(&mut *blocker)
            .await
            .expect("hold effect key");
        let (p, t, ids) = (pool.clone(), f.time.clone(), f.ids.clone());
        let task = tokio::spawn(async move {
            let items = [units("SET-FILLED")];
            let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
            record(
                &p,
                &token(71),
                t.as_ref(),
                ids.as_ref(),
                packed(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "shared-effect",
                    &items,
                    &packages,
                ),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_millis(800),async {
            loop {
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT pg_advisory_xact_lock(hashtextextended(%')").fetch_one(&pool).await.expect("actual effect wait");
                if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("contender waited for effect key");
        if expired {
            f.time.0.store(2351, Ordering::SeqCst);
        }
        blocker.commit().await.expect("release key");
        let result = task.await.expect("task");
        assert!(if expired {
            matches!(result, Err(Error::Denied))
        } else {
            matches!(result, Err(Error::Conflict))
        });
    }
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM packing_custody),(SELECT count(*) FROM commerce_outbox WHERE kind='order.packing_custody_recorded.v1')").fetch_one(&pool).await.expect("unchanged custody");
    assert_eq!(counts, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_commit_wait_cannot_expose_database_outcome_after_mfa_expiry(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = f.order.identity.id.clone();
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &order)
        .await
        .expect("settlement");
    pool.execute("CREATE FUNCTION packing_test_commit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtextextended('packing-test-commit',0)); RAISE EXCEPTION 'test commit failure'; END $$; CREATE CONSTRAINT TRIGGER packing_test_commit_failure AFTER INSERT ON packing_custody DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION packing_test_commit_failure()").await.expect("isolated deferred failure");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    assert!(
        matches!(
            record(
                &pool,
                &token(71),
                f.time.as_ref(),
                f.ids.as_ref(),
                packed(&order, "commit-fresh", &items, &packages)
            )
            .await,
            Err(Error::Database)
        ),
        "fresh authority exposes only the closed database category"
    );
    let mut blocker = pool.begin().await.expect("commit blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('packing-test-commit',0))")
        .execute(&mut *blocker)
        .await
        .expect("hold deferred lock");
    let (p, t, ids, o) = (pool.clone(), f.time.clone(), f.ids.clone(), order.clone());
    let task = tokio::spawn(async move {
        let items = [units("SET-FILLED")];
        let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
        record(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            packed(&o, "commit-expired", &items, &packages),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query='COMMIT')").fetch_one(&pool).await.expect("actual commit wait");
            if waiting { break; } tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("commit reached deferred lock");
    f.time.0.store(2351, Ordering::SeqCst);
    blocker.commit().await.expect("release commit");
    assert!(matches!(task.await.expect("task"), Err(Error::Denied)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("failed commits rolled back");
    assert_eq!(count, 0);
    pool.execute("DROP TRIGGER packing_test_commit_failure ON packing_custody; DROP FUNCTION packing_test_commit_failure()").await.expect("remove test fault");
    f.time.0.store(1500, Ordering::SeqCst);
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            packed(&order, "commit-repaired", &items, &packages)
        )
        .await
        .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn released_record_keeps_target_pack_recognition_across_new_received_evidence(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let first = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "lineage-pack", &items, &packages),
    )
    .await
    .expect("pack")
    .record;
    fixture::observe(&pool, &f, "tr_SETTLEMENT", "paid", "lineage-new-paid").await;
    let latest = crate::payment_attempts::recognition::recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &first.attempt_id,
    )
    .await
    .expect("new received recognition")
    .recognition;
    assert_eq!(
        latest.state,
        crate::payment_attempts::recognition::State::Received
    );
    assert_ne!(first.recognition_id, latest.id);
    let release_record = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "lineage-release", &first.id),
    )
    .await
    .expect("release")
    .record;
    assert_eq!(release_record.recognition_id, first.recognition_id);
    pool.execute("ALTER TABLE packing_custody DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER").await.expect("isolated tamper");
    sqlx::query("UPDATE packing_custody SET recognition_id=$1 WHERE id=$2")
        .bind(&latest.id)
        .bind(&release_record.id)
        .execute(&pool)
        .await
        .expect("substitute other valid received recognition");
    pool.execute("UPDATE commerce_outbox e SET payload=packing_custody_event(p) FROM packing_custody p WHERE e.id=p.event_id; ALTER TABLE packing_custody ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("reseal tamper");
    let valid: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("native lineage check");
    assert!(!valid);
    assert!(matches!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "lineage-release", &first.id)
        )
        .await,
        Err(Error::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE packing_custody DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER").await.expect("isolated repair");
    sqlx::query("UPDATE packing_custody SET recognition_id=$1 WHERE id=$2")
        .bind(&first.recognition_id)
        .bind(&release_record.id)
        .execute(&pool)
        .await
        .expect("restore exact recognition");
    pool.execute("UPDATE commerce_outbox e SET payload=packing_custody_event(p) FROM packing_custody p WHERE e.id=p.event_id; ALTER TABLE packing_custody ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("restore exact event");
    assert!(
        record(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            released(order, "lineage-release", &first.id)
        )
        .await
        .expect("repaired replay")
        .replayed
    );
}

#[sqlx::test(migrations = false)]
async fn populated_schema66_upgrade_preserves_settled_order_and_accepts_packing(pool: PgPool) {
    MIGRATOR.run_to(66, &pool).await.expect("schema 0066");
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    let attempt = &f.frozen.snapshot().binding.id;
    let settlement = settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("real pre-upgrade paid settlement")
        .settlement;
    assert!(
        crate::order_stock::read_settlement(&pool, order)
            .await
            .expect("original reader")
            == Some(settlement.clone())
    );
    let recognition = crate::payment_attempts::recognition::current(&pool, attempt)
        .await
        .expect("original recognition")
        .expect("received");
    assert_eq!(
        recognition.state,
        crate::payment_attempts::recognition::State::Received
    );
    let source = fixture::unchanged(&pool).await;
    let stock = fixture::footprint(&pool).await;
    let old_events: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("pre-upgrade immutable events");
    let original_evidence: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM payment_attempts a),'observations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY o.id) FROM payment_observations o),'resolutions',(SELECT jsonb_agg(to_jsonb(i) ORDER BY i.id) FROM payment_identity_resolutions i),'recognitions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM payment_recognitions r),'current',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.order_id) FROM payment_recognition_current r),'allocations',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.order_id) FROM order_stock_allocations a))")
        .fetch_one(&pool).await.expect("pre-upgrade financial and allocation custody");
    assert!(!old_events.is_empty());

    MIGRATOR.run_to(67, &pool).await.expect("upgrade to 0067");
    assert_eq!(fixture::unchanged(&pool).await, source);
    assert_eq!(fixture::footprint(&pool).await, stock);
    assert_eq!(
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT to_jsonb(e) FROM commerce_outbox e ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .expect("preserved events"),
        old_events
    );
    let upgraded_evidence: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM payment_attempts a),'observations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY o.id) FROM payment_observations o),'resolutions',(SELECT jsonb_agg(to_jsonb(i) ORDER BY i.id) FROM payment_identity_resolutions i),'recognitions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM payment_recognitions r),'current',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.order_id) FROM payment_recognition_current r),'allocations',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.order_id) FROM order_stock_allocations a))")
        .fetch_one(&pool).await.expect("upgraded financial and allocation custody");
    assert_eq!(upgraded_evidence, original_evidence);
    assert!(
        crate::order_stock::read_settlement(&pool, order)
            .await
            .expect("upgraded old reader")
            == Some(settlement.clone())
    );
    assert!(
        crate::payment_attempts::recognition::current(&pool, attempt)
            .await
            .expect("upgraded recognition")
            == Some(recognition)
    );
    let initial_count: i64 = sqlx::query_scalar("SELECT count(*) FROM packing_custody")
        .fetch_one(&pool)
        .await
        .expect("no seeded packing");
    assert_eq!(initial_count, 0);
    let items = [units("SET-FILLED")];
    let packages = [vec![part("SEAT-COVER", 2), part("SEAT-FOAM", 2)]];
    let pack = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packed(order, "upgrade-pack", &items, &packages),
    )
    .await
    .expect("first upgraded pack");
    let release = record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        released(order, "upgrade-release", &pack.record.id),
    )
    .await
    .expect("first upgraded release");
    assert_eq!((pack.record.generation, release.record.generation), (1, 2));
    assert_eq!(fixture::unchanged(&pool).await, source);
    assert_eq!(fixture::footprint(&pool).await, stock);
    assert_eq!(sqlx::query_scalar::<_,serde_json::Value>("SELECT to_jsonb(e) FROM commerce_outbox e WHERE kind<>'order.packing_custody_recorded.v1' ORDER BY id").fetch_all(&pool).await.expect("original events after packing"),old_events);
    assert!(
        crate::order_stock::read_settlement(&pool, order)
            .await
            .expect("old reader after packing")
            == Some(settlement)
    );
    let exact: bool = sqlx::query_scalar("SELECT packing_custody_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("upgraded exact packing custody");
    assert!(exact);
}
