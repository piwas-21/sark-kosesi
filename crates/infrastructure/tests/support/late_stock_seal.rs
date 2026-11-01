//! Full native proof must survive early flush, savepoints and hostile lock callers.
use super::*;
async fn complete(tx: &mut Transaction<'_, Postgres>, s: &LateAllocation) {
    prepare(tx, s).await.expect("fresh construction");
    insert(tx, s).await.expect("receipt before effects");
    effects(tx, s, Fault::Healthy).await;
    event(tx, s).await.expect("exact finished event");
}
async fn append_observation(
    tx: &mut Transaction<'_, Postgres>,
    s: &LateAllocation,
    verified: &VerifiedPayment,
) -> Result<(), sqlx::Error> {
    let observation = commerce_domain::payment_attempts::observations::ObservationSnapshot {
        schema_version: 1,
        id: "e".repeat(32),
        attempt_id: s.attempt_id.clone(),
        effect_key: "after-late-flush".into(),
        revision: 2,
        recorded_at: 2000,
        intent: commerce_domain::payment_attempts::observations::ObservationIntent {
            payment: verified.payment().clone(),
            credential_fingerprint: verified.credential_fingerprint().into(),
        },
    };
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,2,2000,'tr_SETTLEMENT',$4,'open',$5)").bind(&observation.id).bind(&s.attempt_id).bind(&observation.effect_key).bind(&observation.intent.credential_fingerprint).bind(serde_json::to_value(&observation).expect("actual authenticated observation")).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,2000)").bind("f".repeat(32)).bind(&s.order_id).bind(json!({"schema_version":1,"order_id":s.order_id,"attempt_id":s.attempt_id,"observation_id":observation.id})).execute(&mut **tx).await?;
    Ok(())
}
async fn changed_balance(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE stock_balances SET on_hand=on_hand+1 WHERE component_code='SEAT-COVER'")
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_early_flush_then_deferred_financial_pair_or_physical_write_is_rejected_by_completion_seal(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    for financial in [true, false] {
        let mut tx = pool.begin().await.expect("early-flush caller");
        complete(&mut tx, &s).await;
        tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("healthy exact full proof");
        tx.execute("SET CONSTRAINTS ALL DEFERRED")
            .await
            .expect("legitimate multirow source writes can otherwise be staged");
        let error = if financial {
            append_observation(&mut tx, &s, &verified)
                .await
                .expect_err("sealed finance cannot change")
        } else {
            changed_balance(&mut tx)
                .await
                .expect_err("sealed physical source cannot change")
        };
        constraint(&error);
        assert_eq!(
            error
                .as_database_error()
                .expect("native source diagnostic")
                .message(),
            "Completed late allocation source cannot change again in this transaction"
        );
        tx.rollback().await.expect("full rollback");
        assert_eq!(all_effects(&pool).await, before);
        healthy_probe(&pool, &s).await;
    }
    let mut tx = pool.begin().await.expect("healthy early flush commit");
    complete(&mut tx, &s).await;
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("exact proof");
    tx.commit().await.expect("no later relevant mutation");
    receive(
        &pool,
        "later-legal-warehouse",
        "SEAT-COVER",
        WarehouseChange::Receive(1),
    )
    .await;
    f.time.0.store(2000, Ordering::SeqCst);
    record_verified(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &s.attempt_id,
        "later-legal-observation",
        &verified,
    )
    .await
    .expect("separate transaction may append");
    replay_without_ports(&pool, &f, Outcome::Late(s)).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_savepoint_rollback_releases_child_seal_and_restores_pending_proof(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    for financial in [true, false] {
        let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
        let mut tx = pool.begin().await.expect("savepoint caller");
        complete(&mut tx, &s).await;
        tx.execute("SAVEPOINT before_proof")
            .await
            .expect("proof savepoint");
        tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("child seal");
        tx.execute("ROLLBACK TO SAVEPOINT before_proof")
            .await
            .expect("child seal release and pending proof restored");
        if financial {
            append_observation(&mut tx, &s, &verified)
                .await
                .expect("seal rolled back so genuine new source can stage");
        } else {
            changed_balance(&mut tx)
                .await
                .expect("seal rolled back so real balance can stage");
        }
        constraint(
            &tx.commit()
                .await
                .expect_err("restored pending proof sees later source and rejects full commit"),
        );
        assert_eq!(all_effects(&pool).await, before);
        healthy_probe(&pool, &s).await;
    }
    let mut tx = pool.begin().await.expect("healthy savepoint caller");
    complete(&mut tx, &s).await;
    tx.execute("SAVEPOINT before_proof")
        .await
        .expect("proof savepoint");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("child seal");
    tx.execute("ROLLBACK TO SAVEPOINT before_proof")
        .await
        .expect("rollback proof only");
    tx.commit()
        .await
        .expect("restored pending proof healthy commit");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_release_preserves_seal_and_session_unlock_cannot_release_transaction_completion(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    for release in [true, false] {
        let mut tx = pool.begin().await.expect("caller");
        complete(&mut tx, &s).await;
        if release {
            tx.execute("SAVEPOINT proof")
                .await
                .expect("proof savepoint");
        }
        tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("proof seal");
        if release {
            tx.execute("RELEASE SAVEPOINT proof")
                .await
                .expect("release preserves seal");
        } else {
            tx.execute("SELECT pg_advisory_unlock_all()")
                .await
                .expect("session unlock call");
        }
        constraint(
            &changed_balance(&mut tx)
                .await
                .expect_err("transaction completion seal remains"),
        );
        tx.rollback().await.expect("rollback");
        assert_eq!(all_effects(&pool).await, before);
        healthy_probe(&pool, &s).await;
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_manual_matching_advisory_lock_never_skips_full_actual_physical_proof(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = all_effects(&pool).await;
    let mut tx = pool.begin().await.expect("hostile caller");
    prepare(&mut tx, &s).await.expect("fresh source");
    insert(&mut tx, &s).await.expect("receipt admission");
    effects(&mut tx, &s, Fault::NoPhysicalChange).await;
    event(&mut tx, &s)
        .await
        .expect("exact event still lacks physical change");
    sqlx::query("SELECT pg_advisory_xact_lock(1937011564,fence_key) FROM late_stock_allocations WHERE id=$1").bind(&s.id).execute(&mut *tx).await.expect("caller manually owns exact live seal key");
    let error = tx
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect_err("matching advisory key is never an authority token");
    constraint(&error);
    assert_eq!(
        error
            .as_database_error()
            .expect("native completion proof")
            .message(),
        "Late allocation requires complete exact physical stages claim and event"
    );
    tx.rollback().await.expect("rollback counterfeit");
    assert_eq!(all_effects(&pool).await, before);
    healthy_probe(&pool, &s).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_native_sealed_covers_only_transaction_can_receive_unrelated_foam_and_commit(
    pool: PgPool,
) {
    let f = accepted(&pool, true, true).await;
    let mut s = candidate(&pool, &f).await;
    s.footprint.components = vec![StageComponent {
        component_code: "SEAT-COVER".into(),
        demand: 2,
        active: 2,
        before: Balance {
            on_hand: 10,
            quarantine: 1,
            reserved: 2,
        },
        expired: Balance {
            on_hand: 10,
            quarantine: 1,
            reserved: 0,
        },
        reserved: Balance {
            on_hand: 10,
            quarantine: 1,
            reserved: 2,
        },
        after: Balance {
            on_hand: 8,
            quarantine: 1,
            reserved: 0,
        },
    }];
    let mut tx = pool
        .begin()
        .await
        .expect("same sealed transaction positive");
    complete(&mut tx, &s).await;
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("actual complete covers footprint proof");
    tx.execute("UPDATE stock_balances SET on_hand=on_hand+3 WHERE component_code='SEAT-FOAM'; INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after) VALUES('unrelated-foam-after-late-flush','SEAT-FOAM','receive',3,'fixture-warehouse','synthetic physical custody',3,0,0,7,1,0)").await.expect("unrelated physical component remains writable");
    tx.commit()
        .await
        .expect("valid receipt and unrelated warehouse write commit");
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 8, 1, 0),
            ("SEAT-FOAM".into(), 7, 1, 0)
        ]
    );
    replay_without_ports(&pool, &f, Outcome::Late(s)).await;
}
