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
