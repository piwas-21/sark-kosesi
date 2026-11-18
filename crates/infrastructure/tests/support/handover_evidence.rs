use super::*;
use crate::{
    MIGRATOR, mollie::tests::order_stock_cases::fixture, order_stock::settle, packing_custody,
};
use sqlx::Executor;

async fn setup(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES('evidence-staff',true),('no-evidence',true),('second-evidence',true); INSERT INTO staff_capabilities VALUES('evidence-staff','warehouse.manage'),('evidence-staff','handover.record'),('no-evidence','warehouse.manage'),('second-evidence','handover.record')")
        .await.expect("staff");
    for (byte, actor) in [
        (71, "evidence-staff"),
        (72, "no-evidence"),
        (73, "second-evidence"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,8000,1450)")
            .bind(token(byte).digest().as_slice()).bind(actor).execute(pool).await.expect("session");
    }
}
fn token(byte: u8) -> SessionToken {
    SessionToken::from_bytes([byte; 32])
}
fn request<'a>(order: &'a str, parent: &'a str, key: &'a str, bytes: &'a [u8]) -> Request<'a> {
    Request {
        order_id: order,
        packing_id: parent,
        effect_key: key,
        bytes,
        confirmed: true,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn submission_is_historical_and_never_blocks_release(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = &f.order.identity.id;
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), order)
        .await
        .expect("settlement");
    let items = [packing_custody::Units {
        sku: "SET-FILLED".into(),
        units: 1,
    }];
    let packages = [vec![
        packing_custody::Component {
            code: "SEAT-COVER".into(),
            quantity: 2,
        },
        packing_custody::Component {
            code: "SEAT-FOAM".into(),
            quantity: 2,
        },
    ]];
    let packed = packing_custody::record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packing_custody::Request {
            order_id: order,
            effect_key: "evidence-pack",
            confirmed: true,
            action: packing_custody::Action::Packed {
                units: &items,
                packages: &packages,
            },
        },
    )
    .await
    .expect("pack");
    let parent = &packed.record.id;
    let before = fixture::footprint(&pool).await;
    assert!(matches!(
        submit(
            &pool,
            &token(72),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "denied", b"abc")
        )
        .await,
        Err(Error::Denied)
    ));
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "empty", b"")
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    let first = submit(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, parent, "first", b"abc"),
    )
    .await
    .expect("submit");
    assert_eq!(
        first.submission.evidence_sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(!first.replayed);
    assert!(matches!(
        submit(
            &pool,
            &token(73),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "first", b"abc")
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(
                order,
                "cccccccccccccccccccccccccccccccc",
                "wrong-parent",
                b"abc"
            )
        )
        .await,
        Err(Error::Conflict)
    ));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox SET payload='{}' WHERE kind='order.handover_evidence_submitted.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("isolated corruption");
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "first", b"abc")
        )
        .await,
        Err(Error::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox e SET payload=handover_evidence_event(s) FROM handover_evidence_submissions s WHERE e.id=s.event_id AND e.kind='order.handover_evidence_submitted.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("exact repair");
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "first", b"different")
        )
        .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "duplicate", b"abc")
        )
        .await,
        Err(Error::Conflict)
    ));
    // A native row without its reciprocal event must not commit.
    let mut detached = pool.begin().await.expect("native transaction");
    let mut second = first.submission.clone();
    second.id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    second.event_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    second.effect_key = "detached".into();
    second.generation = 2;
    second.evidence_sha256 = "c".repeat(64);
    sqlx::query("INSERT INTO handover_evidence_submissions(id,order_id,attempt_id,packing_id,actor_id,effect_key,generation,evidence_sha256,recorded_at,event_id,transaction_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,pg_current_xact_id())")
        .bind(&second.id).bind(&second.order_id).bind(&second.attempt_id).bind(&second.packing_id)
        .bind(&second.actor_id).bind(&second.effect_key).bind(second.generation).bind(&second.evidence_sha256)
        .bind(second.recorded_at).bind(&second.event_id).execute(&mut *detached).await.expect("provisional native row");
    assert!(detached.commit().await.is_err());
    // Hold a complete evidence transaction while release attempts the same claim.
    second.effect_key = "race-evidence".into();
    let mut evidence_tx = pool.begin().await.expect("evidence transaction");
    store::insert(&mut evidence_tx, &second)
        .await
        .expect("native reciprocal insert");
    let (p, t, ids, parent_owned, order_owned) = (
        pool.clone(),
        f.time.clone(),
        f.ids.clone(),
        parent.clone(),
        order.clone(),
    );
    let waiter = tokio::spawn(async move {
        packing_custody::record(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            packing_custody::Request {
                order_id: &order_owned,
                effect_key: "evidence-release",
                confirmed: true,
                action: packing_custody::Action::Released {
                    target_id: &parent_owned,
                },
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    evidence_tx.commit().await.expect("evidence wins");
    waiter
        .await
        .expect("release task")
        .expect("release remains permitted");
    let replay = submit(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, parent, "first", b"abc"),
    )
    .await
    .expect("historical replay");
    assert!(replay.replayed);
    let direct = sqlx::query("INSERT INTO handover_evidence_submissions(id,order_id,attempt_id,packing_id,actor_id,effect_key,generation,evidence_sha256,recorded_at,event_id,transaction_id) VALUES('dddddddddddddddddddddddddddddddd',$1,$2,$3,'evidence-staff','direct-after-release',3,repeat('d',64),$4,'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',pg_current_xact_id())")
        .bind(order).bind(&first.submission.attempt_id).bind(parent).bind(first.submission.recorded_at).execute(&pool).await;
    assert!(
        direct.is_err(),
        "native guard must reject a released parent"
    );
    assert_eq!(first.submission, replay.submission);
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "after-release", b"other")
        )
        .await,
        Err(Error::Conflict)
    ));
    let found = inspect(&pool, &token(71), f.time.as_ref(), order)
        .await
        .expect("history");
    assert_eq!(found, vec![(first.submission, true), (second, true)]);
    assert_eq!(before, fixture::footprint(&pool).await);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn release_wins_the_attempt_claim_before_submission(pool: PgPool) {
    let f = fixture::accepted(&pool, false, true).await;
    setup(&pool).await;
    let order = f.order.identity.id.clone();
    settle(&pool, f.time.as_ref(), f.ids.as_ref(), &order)
        .await
        .expect("settlement");
    let items = [packing_custody::Units {
        sku: "SET-FILLED".into(),
        units: 1,
    }];
    let packages = [vec![
        packing_custody::Component {
            code: "SEAT-COVER".into(),
            quantity: 2,
        },
        packing_custody::Component {
            code: "SEAT-FOAM".into(),
            quantity: 2,
        },
    ]];
    let packed = packing_custody::record(
        &pool,
        &token(71),
        f.time.as_ref(),
        f.ids.as_ref(),
        packing_custody::Request {
            order_id: &order,
            effect_key: "race-pack",
            confirmed: true,
            action: packing_custody::Action::Packed {
                units: &items,
                packages: &packages,
            },
        },
    )
    .await
    .expect("pack")
    .record;
    let mut release = packed.clone();
    release.id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into();
    release.event_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    release.effect_key = "race-release".into();
    release.generation = 2;
    release.action = "released".into();
    release.target_id = Some(packed.id.clone());
    release.units = sqlx::types::Json(Vec::new());
    release.packages = sqlx::types::Json(Vec::new());
    let mut tx = pool.begin().await.expect("transaction");
    packing_custody::store::insert(&mut tx, &release)
        .await
        .expect("hold release claim");
    let (p, t, ids, parent, o) = (
        pool.clone(),
        f.time.clone(),
        f.ids.clone(),
        packed.id.clone(),
        order.clone(),
    );
    let waiter = tokio::spawn(async move {
        submit(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            request(&o, &parent, "race-submission", b"source"),
        )
        .await
    });
    tokio::task::yield_now().await;
    tx.commit().await.expect("release wins");
    assert!(matches!(waiter.await.expect("task"), Err(Error::Conflict)));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM handover_evidence_submissions WHERE order_id=$1")
            .bind(order)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 0);
}
