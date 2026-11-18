use super::*;
use crate::{
    MIGRATOR, mollie::tests::order_stock_cases::fixture, order_stock::settle, packing_custody,
};
use sqlx::Executor;
use std::sync::atomic::Ordering;

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
async fn wait_for_lock(pool: &PgPool, blocker_pid: i32, query: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_stat_activity a WHERE a.datname=pg_catalog.current_database() AND a.wait_event_type='Lock' AND $1=ANY(pg_catalog.pg_blocking_pids(a.pid)) AND a.query LIKE $2)")
                .bind(blocker_pid).bind(query).fetch_one(pool).await.expect("waiter probe");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("worker reached the held lock");
}
async fn wait_for_attempt_claim(pool: &PgPool, blocker_pid: i32) {
    wait_for_lock(
        pool,
        blocker_pid,
        "SELECT id FROM payment_attempts WHERE id=%",
    )
    .await;
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
    // A grant alone does not replace a valid current MFA session.
    for (mfa, revoked) in [
        (None, None),
        (Some(1501_i64), None),
        (Some(1450_i64), Some(1500_i64)),
    ] {
        sqlx::query("UPDATE public.staff_sessions SET mfa_verified_at=$1, revoked_at=$2 WHERE staff_id='second-evidence'")
            .bind(mfa).bind(revoked).execute(&pool).await.expect("session state");
        assert!(matches!(
            submit(
                &pool,
                &token(73),
                f.time.as_ref(),
                f.ids.as_ref(),
                request(order, parent, "invalid-session", b"abc")
            )
            .await,
            Err(Error::Denied)
        ));
    }
    sqlx::query("UPDATE public.staff_sessions SET mfa_verified_at=1450, revoked_at=NULL WHERE staff_id='second-evidence'")
        .execute(&pool).await.expect("restore session");
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
    // MFA remains valid: a clock before the recorded evidence must still fail closed.
    f.time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "first", b"abc")
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), order).await,
        Err(Error::ClockUnavailable)
    ));
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "rewind-new", b"other")
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    for (target, key, bytes) in [
        (parent.as_str(), "first", b"different".as_slice()),
        (parent.as_str(), "duplicate-before", b"abc".as_slice()),
        (
            "cccccccccccccccccccccccccccccccc",
            "first",
            b"abc".as_slice(),
        ),
        (
            "cccccccccccccccccccccccccccccccc",
            "new-parent",
            b"abc".as_slice(),
        ),
    ] {
        assert!(matches!(
            submit(
                &pool,
                &token(71),
                f.time.as_ref(),
                f.ids.as_ref(),
                request(order, target, key, bytes)
            )
            .await,
            Err(Error::ClockUnavailable)
        ));
    }
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request("ffffffffffffffffffffffffffffffff", parent, "first", b"abc")
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    f.time.0.store(1500, Ordering::SeqCst);
    assert!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "first", b"abc")
        )
        .await
        .expect("exact evidence time")
        .replayed
    );
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
    for (target, key, bytes) in [
        (parent.as_str(), "first", b"different".as_slice()),
        (parent.as_str(), "duplicate-before", b"abc".as_slice()),
        (
            "cccccccccccccccccccccccccccccccc",
            "first",
            b"abc".as_slice(),
        ),
        (
            "cccccccccccccccccccccccccccccccc",
            "new-parent",
            b"abc".as_slice(),
        ),
    ] {
        assert!(matches!(
            submit(
                &pool,
                &token(71),
                f.time.as_ref(),
                f.ids.as_ref(),
                request(order, target, key, bytes)
            )
            .await,
            Err(Error::Conflict)
        ));
    }
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request("ffffffffffffffffffffffffffffffff", parent, "first", b"abc")
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
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_catalog.pg_backend_pid()")
        .fetch_one(&mut *evidence_tx)
        .await
        .expect("evidence backend");
    f.time.0.store(1600, Ordering::SeqCst);
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
    wait_for_attempt_claim(&pool, blocker_pid).await;
    evidence_tx.commit().await.expect("evidence wins");
    waiter
        .await
        .expect("release task")
        .expect("release remains permitted");
    // Release is timestamped at 1600; rewinding must not reveal its presence.
    f.time.0.store(1500, Ordering::SeqCst);
    assert!(matches!(
        inspect(&pool, &token(71), f.time.as_ref(), order).await,
        Err(Error::ClockUnavailable)
    ));
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "first", b"abc")
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "new-after-release", b"new")
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    f.time.0.store(1600, Ordering::SeqCst);
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, parent, "new-after-release", b"new")
        )
        .await,
        Err(Error::Conflict)
    ));
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
    // The two writes must never share a transaction, even after an early flush.
    let mut candidate = Submission {
        id: "dddddddddddddddddddddddddddddddd".into(),
        order_id: order.clone(),
        attempt_id: packed.attempt_id.clone(),
        packing_id: packed.id.clone(),
        actor_id: "evidence-staff".into(),
        effect_key: "rolled-key".into(),
        generation: 1,
        evidence_sha256: "c".repeat(64),
        recorded_at: packed.recorded_at,
        event_id: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
    };
    let mut same = pool.begin().await.expect("submission first");
    store::insert(&mut same, &candidate)
        .await
        .expect("early evidence flush");
    assert!(
        packing_custody::store::insert(&mut same, &release)
            .await
            .is_err()
    );
    same.rollback().await.expect("roll back both");
    let mut same = pool.begin().await.expect("release first");
    packing_custody::store::insert(&mut same, &release)
        .await
        .expect("early release flush");
    assert!(store::insert(&mut same, &candidate).await.is_err());
    same.rollback().await.expect("roll back both");
    let mut retry = pool.begin().await.expect("retry after rollback");
    store::insert(&mut retry, &candidate)
        .await
        .expect("same key remains reusable after rollback");
    retry.rollback().await.expect("roll back retry");

    // Expiry occurs while the attempt is observably locked, before any write.
    let mut blocker = pool.begin().await.expect("MFA blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&packed.attempt_id)
        .execute(&mut *blocker)
        .await
        .expect("hold attempt");
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_catalog.pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("blocker backend");
    let (p, t, ids, parent, o) = (
        pool.clone(),
        f.time.clone(),
        f.ids.clone(),
        packed.id.clone(),
        order.clone(),
    );
    let expired = tokio::spawn(async move {
        submit(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            request(&o, &parent, "expired-wait", b"source"),
        )
        .await
    });
    wait_for_attempt_claim(&pool, blocker_pid).await;
    f.time.0.store(2351, Ordering::SeqCst);
    blocker.commit().await.expect("release blocker");
    assert!(matches!(expired.await.expect("task"), Err(Error::Denied)));
    f.time.0.store(1500, Ordering::SeqCst);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.handover_evidence_submissions WHERE order_id=$1",
    )
    .bind(&order)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 0);

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
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_catalog.pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .expect("release backend");
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
    wait_for_attempt_claim(&pool, blocker_pid).await;
    tx.commit().await.expect("release wins");
    assert!(matches!(waiter.await.expect("task"), Err(Error::Conflict)));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM handover_evidence_submissions WHERE order_id=$1")
            .bind(&order)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 0);
    // A newly inserted parent remains inadmissible in its own transaction even
    // after packing's deferred event check has been forced and sealed.
    let mut new_pack = packed.clone();
    new_pack.id = "11111111111111111111111111111111".into();
    new_pack.event_id = "22222222222222222222222222222222".into();
    new_pack.effect_key = "new-pack".into();
    new_pack.generation = 3;
    candidate.packing_id = new_pack.id.clone();
    let mut new_parent = pool.begin().await.expect("new parent transaction");
    packing_custody::store::insert(&mut new_parent, &new_pack)
        .await
        .expect("flush new pack");
    assert!(store::insert(&mut new_parent, &candidate).await.is_err());
    new_parent.rollback().await.expect("roll back new parent");
    let mut committed_parent = pool.begin().await.expect("committed parent transaction");
    packing_custody::store::insert(&mut committed_parent, &new_pack)
        .await
        .expect("pack again");
    committed_parent.commit().await.expect("parent committed");
    let mut accepted = pool.begin().await.expect("later transaction");
    store::insert(&mut accepted, &candidate)
        .await
        .expect("later evidence is accepted");
    accepted.commit().await.expect("evidence committed");
    // A simultaneous identical key waits for the original, then replays its exact receipt.
    let mut concurrent = candidate.clone();
    concurrent.id = "33333333333333333333333333333333".into();
    concurrent.event_id = "44444444444444444444444444444444".into();
    concurrent.effect_key = "same-race".into();
    concurrent.generation = 2;
    concurrent.evidence_sha256 = format!("{:x}", Sha256::digest(b"same bytes"));
    let mut first_tx = pool.begin().await.expect("same-key first transaction");
    store::lock_key(&mut first_tx, &concurrent.effect_key)
        .await
        .expect("same-key claim");
    store::insert(&mut first_tx, &concurrent)
        .await
        .expect("same-key first submission");
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_catalog.pg_backend_pid()")
        .fetch_one(&mut *first_tx)
        .await
        .expect("same-key backend");
    let (p, t, ids, parent, o) = (
        pool.clone(),
        f.time.clone(),
        f.ids.clone(),
        new_pack.id.clone(),
        order.clone(),
    );
    let same = tokio::spawn(async move {
        submit(
            &p,
            &token(71),
            t.as_ref(),
            ids.as_ref(),
            request(&o, &parent, "same-race", b"same bytes"),
        )
        .await
    });
    wait_for_lock(
        &pool,
        blocker_pid,
        "SELECT pg_catalog.pg_advisory_xact_lock%",
    )
    .await;
    first_tx.commit().await.expect("first same-key commit");
    let same = same.await.expect("same-key task").expect("same-key replay");
    assert!(same.replayed);
    assert_eq!(same.submission, concurrent);
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&order, &new_pack.id, "too-large", &vec![0; MAX_BYTES + 1])
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    let mut unconfirmed = request(&order, &new_pack.id, "unconfirmed", b"one");
    unconfirmed.confirmed = false;
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            unconfirmed
        )
        .await,
        Err(Error::InvalidRequest)
    ));
    for index in 0..2 {
        let key = format!("more-{index}");
        let bytes = if index == 0 {
            vec![7; MAX_BYTES]
        } else {
            format!("source-{index}").into_bytes()
        };
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&order, &new_pack.id, &key, &bytes),
        )
        .await
        .expect("within parent bound");
    }
    let inspected = inspect(&pool, &token(71), f.time.as_ref(), &order)
        .await
        .expect("bounded inspection");
    assert_eq!(inspected.len(), 4);
    assert!(inspected.iter().all(|(_, released)| !released));
    f.time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&order, &new_pack.id, "fifth", b"fifth")
        )
        .await,
        Err(Error::ClockUnavailable)
    ));
    f.time.0.store(1500, Ordering::SeqCst);
    assert!(matches!(
        submit(
            &pool,
            &token(71),
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&order, &new_pack.id, "fifth", b"fifth")
        )
        .await,
        Err(Error::Conflict)
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.handover_evidence_submissions WHERE packing_id=$1",
    )
    .bind(&new_pack.id)
    .fetch_one(&pool)
    .await
    .expect("bounded count");
    assert_eq!(count, 4);
    // Existing packing writers need no read access to the new evidence table.
    pool.execute("CREATE ROLE handover_limited_packer NOLOGIN; GRANT USAGE ON SCHEMA public TO handover_limited_packer; GRANT SELECT ON ALL TABLES IN SCHEMA public TO handover_limited_packer; REVOKE SELECT ON public.handover_evidence_submissions FROM handover_limited_packer; GRANT INSERT ON public.packing_custody,public.commerce_outbox TO handover_limited_packer; GRANT UPDATE ON public.payment_attempts TO handover_limited_packer; GRANT USAGE ON SEQUENCE public.packing_custody_fence_key_seq TO handover_limited_packer")
        .await.expect("isolated baseline packing permissions");
    let allowed: bool = sqlx::query_scalar("SELECT pg_catalog.has_table_privilege('handover_limited_packer','public.handover_evidence_submissions','SELECT')")
        .fetch_one(&pool).await.expect("role privileges");
    assert!(!allowed);
    let mut limited_release = new_pack.clone();
    limited_release.id = "55555555555555555555555555555555".into();
    limited_release.event_id = "66666666666666666666666666666666".into();
    limited_release.effect_key = "limited-release".into();
    limited_release.generation = 4;
    limited_release.action = "released".into();
    limited_release.target_id = Some(new_pack.id.clone());
    limited_release.units = sqlx::types::Json(Vec::new());
    limited_release.packages = sqlx::types::Json(Vec::new());
    let mut limited = pool.begin().await.expect("restricted packing transaction");
    sqlx::query("SET LOCAL ROLE handover_limited_packer")
        .execute(&mut *limited)
        .await
        .expect("restricted role");
    packing_custody::store::insert(&mut limited, &limited_release)
        .await
        .expect("restricted writer can release");
    limited.commit().await.expect("restricted release commits");
}
