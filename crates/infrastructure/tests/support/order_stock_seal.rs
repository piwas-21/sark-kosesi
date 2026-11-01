//! Early deferred-proof flush cannot authorize later same-transaction mutations.
use super::*;
async fn late_observation(
    tx: &mut Transaction<'_, Postgres>,
    s: &Settlement,
    verified: &VerifiedPayment,
) -> Result<(), sqlx::Error> {
    let observation = commerce_domain::payment_attempts::observations::ObservationSnapshot {
        schema_version: 1,
        id: "c".repeat(32),
        attempt_id: s.attempt_id.clone(),
        effect_key: "after-early-flush".into(),
        revision: 2,
        recorded_at: 1500,
        intent: commerce_domain::payment_attempts::observations::ObservationIntent {
            payment: verified.payment().clone(),
            credential_fingerprint: verified.credential_fingerprint().into(),
        },
    };
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,2,1500,'tr_SETTLEMENT',$4,'open',$5)").bind(&observation.id).bind(&s.attempt_id).bind(&observation.effect_key).bind(&observation.intent.credential_fingerprint).bind(serde_json::to_value(&observation).expect("genuine late observation snapshot")).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1500)").bind("d".repeat(32)).bind(&s.order_id).bind(json!({"schema_version":1,"order_id":s.order_id,"attempt_id":s.attempt_id,"observation_id":observation.id})).execute(&mut **tx).await?;
    Ok(())
}
async fn changed_balance(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE stock_balances SET on_hand=on_hand+1 WHERE component_code='SEAT-COVER'")
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
async fn prepare_complete(tx: &mut Transaction<'_, Postgres>, s: &Settlement) {
    insert(tx, s).await.expect("prepared active receipt");
    effects(tx, s, Fault::Healthy).await;
    event(tx, s)
        .await
        .expect("complete actual canonical effects and event");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_early_constraint_flush_rejects_later_financial_and_affected_balance_writes(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    for financial in [true, false] {
        let mut tx = pool.begin().await.expect("raw early-flush caller");
        prepare_complete(&mut tx, &s).await;
        tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("genuine complete proof flush");
        if financial {
            tx.execute("SET CONSTRAINTS ALL DEFERRED")
                .await
                .expect("later source pair may otherwise be assembled under deferred completeness");
        }
        let error = if financial {
            late_observation(&mut tx, &s, &verified)
                .await
                .expect_err("completed transaction must reject later observation and event")
        } else {
            changed_balance(&mut tx)
                .await
                .expect_err("completed transaction must reject affected physical mutation")
        };
        constraint(&error);
        assert_eq!(
            error
                .as_database_error()
                .expect("specific completion seal guard")
                .message(),
            "Completed settlement source cannot change again in this transaction"
        );
        tx.rollback()
            .await
            .expect("guard failure rolls back complete operation");
        assert_eq!(footprint(&pool).await, before);
        let mut healthy = pool
            .begin()
            .await
            .expect("paired identical healthy early-flush control");
        prepare_complete(&mut healthy, &s).await;
        healthy
            .execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("same proof genuinely succeeds");
        if financial {
            healthy
                .execute("SET CONSTRAINTS ALL DEFERRED")
                .await
                .expect("changing completeness mode alone is harmless");
        }
        healthy
            .rollback()
            .await
            .expect("no canonical row masks next case");
        assert_eq!(footprint(&pool).await, before);
    }
    let mut healthy = pool.begin().await.expect("healthy early flush can commit");
    prepare_complete(&mut healthy, &s).await;
    healthy
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("complete proof");
    healthy
        .commit()
        .await
        .expect("no later mutation permits commit");
    receive(
        &pool,
        "after-completion-seal",
        "SEAT-COVER",
        commerce_domain::inventory::WarehouseChange::Receive(1),
    )
    .await;
    record_verified(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &s.attempt_id,
        "new-transaction-open",
        &verified,
    )
    .await
    .expect("later separate transaction may append legitimate evidence");
    assert!(
        read_settlement(&pool, &s.order_id)
            .await
            .expect("historical replay remains valid after new transactions")
            == Some(s)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_savepoint_rollback_removes_completion_seal_and_restores_pending_final_proof(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    for financial in [true, false] {
        let mut tx = pool.begin().await.expect("savepoint proof caller");
        prepare_complete(&mut tx, &s).await;
        tx.execute("SAVEPOINT before_final_flush")
            .await
            .expect("savepoint before deferred proof");
        tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("seal acquired inside savepoint");
        tx.execute("ROLLBACK TO SAVEPOINT before_final_flush")
            .await
            .expect("subtransaction rollback releases acquired seal");
        if financial {
            late_observation(&mut tx, &s, &verified)
                .await
                .expect("rollback removed seal so a genuine new observation can be staged");
        } else {
            changed_balance(&mut tx)
                .await
                .expect("rollback removed seal so affected balance can be staged");
        }
        constraint(&tx.commit().await.expect_err(
            "rollback to savepoint must restore pending final proof and reject changed custody",
        ));
        assert_eq!(footprint(&pool).await, before);
    }
    let mut healthy = pool
        .begin()
        .await
        .expect("healthy savepoint rollback control");
    prepare_complete(&mut healthy, &s).await;
    healthy
        .execute("SAVEPOINT healthy_flush")
        .await
        .expect("savepoint");
    healthy
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("initial complete proof");
    healthy
        .execute("ROLLBACK TO SAVEPOINT healthy_flush")
        .await
        .expect("undo flush");
    healthy
        .commit()
        .await
        .expect("restored pending complete proof still permits healthy commit");
    assert!(
        read_settlement(&pool, &s.order_id)
            .await
            .expect("committed healthy original")
            == Some(s)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_savepoint_release_and_session_advisory_unlock_do_not_remove_completion_seal(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let verified = fixture::verified(&f.frozen, "tr_SETTLEMENT", "open").await;
    for financial in [true, false] {
        let mut tx = pool.begin().await.expect("released-savepoint proof caller");
        prepare_complete(&mut tx, &s).await;
        tx.execute("SAVEPOINT keep_completed_flush")
            .await
            .expect("savepoint");
        tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("completed native proof");
        tx.execute("RELEASE SAVEPOINT keep_completed_flush")
            .await
            .expect("promote completion into parent transaction");
        tx.execute("SELECT pg_advisory_unlock_all()")
            .await
            .expect("session advisory unlock cannot remove a transaction seal");
        let error = if financial {
            late_observation(&mut tx, &s, &verified)
                .await
                .expect_err("released savepoint retains financial completion seal")
        } else {
            changed_balance(&mut tx)
                .await
                .expect_err("released savepoint retains physical completion seal")
        };
        constraint(&error);
        tx.rollback().await.expect("rollback rejected mutation");
        assert_eq!(footprint(&pool).await, before);
    }
    let mut healthy = pool
        .begin()
        .await
        .expect("healthy released-savepoint control");
    prepare_complete(&mut healthy, &s).await;
    healthy
        .execute("SAVEPOINT healthy_release")
        .await
        .expect("savepoint");
    healthy
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("complete proof");
    healthy
        .execute("RELEASE SAVEPOINT healthy_release")
        .await
        .expect("preserve completion");
    healthy
        .execute("SELECT pg_advisory_unlock_all()")
        .await
        .expect("harmless session unlock");
    healthy
        .commit()
        .await
        .expect("unchanged complete custody can commit");
    assert!(
        read_settlement(&pool, &s.order_id)
            .await
            .expect("healthy released-savepoint receipt")
            == Some(s)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn manually_preacquired_matching_advisory_seal_never_skips_full_physical_proof(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let s = candidate(&pool, &f).await;
    let before = footprint(&pool).await;
    let mut forged = pool
        .begin()
        .await
        .expect("manual seal cannot grant authority");
    insert(&mut forged, &s)
        .await
        .expect("genuine active parent");
    effects(&mut forged, &s, Fault::NoPhysicalChange).await;
    event(&mut forged, &s)
        .await
        .expect("shaped event remains insufficient physical proof");
    sqlx::query("SELECT pg_advisory_xact_lock(1937011563,fence_key) FROM order_stock_settlements WHERE id=$1").bind(&s.id).execute(&mut *forged).await.expect("caller manually acquires exact native namespace and generated key");
    constraint(
        &forged
            .execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect_err("preowned matching lock must not skip complete actual-balance validation"),
    );
    forged
        .rollback()
        .await
        .expect("rollback false physical custody");
    assert_eq!(footprint(&pool).await, before);
    let mut healthy = pool
        .begin()
        .await
        .expect("same manual seal with complete actual proof");
    prepare_complete(&mut healthy, &s).await;
    sqlx::query("SELECT pg_advisory_xact_lock(1937011563,fence_key) FROM order_stock_settlements WHERE id=$1").bind(&s.id).execute(&mut *healthy).await.expect("same caller-selected matching seal");
    healthy
        .execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("valid physical proof still checked");
    healthy
        .commit()
        .await
        .expect("complete healthy custody commits");
    assert!(
        read_settlement(&pool, &s.order_id)
            .await
            .expect("manually sealed healthy history")
            == Some(s)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_completed_covers_only_settlement_allows_unrelated_foam_receipt_in_same_transaction(
    pool: PgPool,
) {
    let f = accepted(&pool, true, true).await;
    let mut s = candidate(&pool, &f).await;
    s.footprint.components = vec![Component {
        component_code: "SEAT-COVER".into(),
        quantity: 2,
        before: Balance {
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
        .expect("complete covers-only stock caller");
    prepare_complete(&mut tx, &s).await;
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("covers-only completion is sealed");
    let changed =
        sqlx::query("UPDATE stock_balances SET on_hand=on_hand+3 WHERE component_code='SEAT-FOAM'")
            .execute(&mut *tx)
            .await
            .expect("unrelated physical component may change after the covers-only seal");
    assert_eq!(changed.rows_affected(), 1);
    sqlx::query("INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after) VALUES('same-sealed-unrelated-foam','SEAT-FOAM','receive',3,'fixture-warehouse','unrelated warehouse receipt',3,0,0,7,1,0)").execute(&mut *tx).await.expect("matching genuine unrelated receive ledger is permitted in same sealed transaction");
    tx.commit()
        .await
        .expect("exact stock settlement and unrelated warehouse receipt commit together");
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 8, 1, 0),
            ("SEAT-FOAM".into(), 7, 1, 0)
        ]
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    let received:i64=sqlx::query_scalar("SELECT count(*) FROM stock_movements WHERE effect_key='same-sealed-unrelated-foam' AND action='receive' AND quantity=3 AND on_hand_delta=3 AND quarantine_delta=0 AND reserved_delta=0 AND on_hand_after=7 AND quarantine_after=1 AND reserved_after=0 AND hold_id IS NULL").fetch_one(&pool).await.expect("actual valid unrelated physical ledger control");
    assert_eq!(received, 1);
    assert!(
        read_settlement(&pool, &s.order_id)
            .await
            .expect("unrelated component is outside historical settlement footprint")
            == Some(s)
    );
}
