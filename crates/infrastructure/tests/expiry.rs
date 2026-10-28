use commerce_domain::inventory::WarehouseChange;
use commerce_infrastructure::{
    MIGRATOR,
    holds::{
        ExpiryBatch, HoldCommand, HoldRequest, HoldWriteError, expire_due_holds, reserve_bom,
        transition_hold,
    },
    stock::{WarehouseRequest, warehouse_movement},
};
use sqlx::{Executor, PgPool};

fn id(n: u32) -> String {
    format!("{n:032x}")
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("fixture");
    for component in ["DEMO-COVER", "DEMO-FOAM"] {
        warehouse_movement(
            pool,
            WarehouseRequest {
                effect_key: component,
                component_code: component,
                change: WarehouseChange::Receive(100),
                actor: "fixture-warehouse",
                reason: "isolated expiry fixture",
            },
        )
        .await
        .expect("receive");
    }
}
async fn hold(pool: &PgPool, n: u32, sku: &str, expires: i64) {
    reserve_bom(
        pool,
        HoldRequest {
            id: &id(n),
            owner_id: "fixture-owner",
            sku,
            revision: 1,
            units: 1,
            created_at: 1000,
            expires_at: expires,
        },
    )
    .await
    .expect("hold");
}
async fn state(pool: &PgPool, n: u32) -> String {
    let (state,): (String,) = sqlx::query_as("SELECT state FROM stock_holds WHERE id=$1")
        .bind(id(n))
        .fetch_one(pool)
        .await
        .expect("state");
    state
}
async fn conserved(pool: &PgPool) {
    let (mismatches,):(i64,)=sqlx::query_as("SELECT count(*) FROM stock_balances b WHERE (b.on_hand,b.quarantine,b.reserved) IS DISTINCT FROM (SELECT ROW(sum(on_hand_delta)::bigint,sum(quarantine_delta)::bigint,sum(reserved_delta)::bigint) FROM stock_movements m WHERE m.component_code=b.component_code)")
        .fetch_one(pool).await.expect("independent conservation oracle");
    assert_eq!(mismatches, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn batch_size_deadline_order_and_repeated_passes_are_exact(pool: PgPool) {
    seed(&pool).await;
    for (n, expiry) in [(3, 1110), (2, 1100), (1, 1100), (4, 1111)] {
        hold(&pool, n, "DEMO-COVERS", expiry).await;
    }
    assert_eq!(
        expire_due_holds(&pool, 1100, 1).await.expect("one"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    assert_eq!(state(&pool, 1).await, "expired");
    assert_eq!(state(&pool, 2).await, "active");
    assert_eq!(
        expire_due_holds(&pool, 1110, 100)
            .await
            .expect("bounded maximum"),
        ExpiryBatch {
            expired: 2,
            failed: 0
        }
    );
    assert_eq!(state(&pool, 4).await, "active", "future hold untouched");
    assert_eq!(
        expire_due_holds(&pool, 1110, 100).await.expect("repeat"),
        ExpiryBatch::default()
    );
    assert_eq!(
        expire_due_holds(&pool, 1111, 25)
            .await
            .expect("exact deadline"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    let balances:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code").fetch_all(&pool).await.expect("balances");
    assert_eq!(
        balances,
        vec![
            ("DEMO-COVER".into(), 100, 0, 0),
            ("DEMO-FOAM".into(), 100, 0, 0)
        ]
    );
    let (audited,):(i64,)=sqlx::query_as("SELECT count(*) FROM stock_movements WHERE action='expire-hold' AND actor='system:hold-expiry'").fetch_one(&pool).await.expect("worker audit");
    assert_eq!(audited, 4);
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn locked_hold_is_skipped_and_concurrent_workers_do_not_double_release(pool: PgPool) {
    seed(&pool).await;
    for n in 1..=6 {
        hold(&pool, n, "DEMO-FILLED", 1100).await;
    }
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM stock_holds WHERE id=$1 FOR UPDATE")
        .bind(id(1))
        .execute(&mut *blocker)
        .await
        .expect("hold lock");
    assert_eq!(
        expire_due_holds(&pool, 1100, 2).await.expect("skip lock"),
        ExpiryBatch {
            expired: 2,
            failed: 0
        }
    );
    assert_eq!(state(&pool, 1).await, "active");
    blocker.rollback().await.expect("release");
    let first = pool.clone();
    let second = pool.clone();
    let a = tokio::spawn(async move { expire_due_holds(&first, 1100, 100).await });
    let b = tokio::spawn(async move { expire_due_holds(&second, 1100, 100).await });
    let a = a.await.expect("worker").expect("batch");
    let b = b.await.expect("worker").expect("batch");
    assert_eq!(a.expired + b.expired, 4);
    assert_eq!(a.failed + b.failed, 0);
    let (effects,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM stock_movements WHERE action='expire-hold'")
            .fetch_one(&pool)
            .await
            .expect("effect count");
    assert_eq!(effects, 12, "each of six holds has two component effects");
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_partial_release_is_rolled_back_deferred_and_recovered_without_starvation(
    pool: PgPool,
) {
    seed(&pool).await;
    for n in 1..=3 {
        hold(&pool, n, "DEMO-FILLED", 1100).await;
    }
    pool.execute("CREATE FUNCTION reject_test_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.hold_id='00000000000000000000000000000001' AND NEW.action='expire-hold' AND NEW.component_code='DEMO-FOAM' THEN RAISE EXCEPTION 'private fixture failure detail'; END IF; RETURN NEW; END; $$;
        CREATE TRIGGER reject_test_expiry BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION reject_test_expiry()")
        .await.expect("partial failure control");
    assert_eq!(
        expire_due_holds(&pool, 1100, 25)
            .await
            .expect("batch continues"),
        ExpiryBatch {
            expired: 2,
            failed: 1
        }
    );
    assert_eq!(state(&pool, 1).await, "active");
    assert_eq!(state(&pool, 2).await, "expired");
    let reservations: Vec<(i64, i64)> =
        sqlx::query_as("SELECT on_hand,reserved FROM stock_balances ORDER BY component_code")
            .fetch_all(&pool)
            .await
            .expect("no partial release");
    assert_eq!(reservations, vec![(100, 2), (100, 2)]);
    let failure:(i32,i64,i64,String,Option<i64>)=sqlx::query_as("SELECT attempts,last_attempt_at,next_attempt_at,code,resolved_at FROM stock_hold_expiry_failures WHERE hold_id=$1").bind(id(1)).fetch_one(&pool).await.expect("retry state");
    assert_eq!(failure, (1, 1100, 1160, "database_error".into(), None));
    assert_eq!(
        expire_due_holds(&pool, 1159, 25).await.expect("not due"),
        ExpiryBatch::default()
    );
    assert_eq!(
        expire_due_holds(&pool, 1160, 25).await.expect("retry"),
        ExpiryBatch {
            expired: 0,
            failed: 1
        }
    );
    pool.execute("DROP TRIGGER reject_test_expiry ON stock_movements")
        .await
        .expect("repair");
    assert_eq!(
        expire_due_holds(&pool, 1220, 25).await.expect("recovery"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    let recovered: (i32, Option<i64>) = sqlx::query_as(
        "SELECT attempts,resolved_at FROM stock_hold_expiry_failures WHERE hold_id=$1",
    )
    .bind(id(1))
    .fetch_one(&pool)
    .await
    .expect("retained failure audit");
    assert_eq!(recovered, (2, Some(1220)));
    assert_eq!(
        expire_due_holds(&pool, 1221, 25).await.expect("replay"),
        ExpiryBatch::default()
    );
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_stock_and_window_failures_have_static_codes_and_can_be_resolved(pool: PgPool) {
    seed(&pool).await;
    hold(&pool, 1, "DEMO-COVERS", 1100).await;
    pool.execute("UPDATE stock_balances SET reserved=0 WHERE component_code='DEMO-COVER'")
        .await
        .expect("corrupt reservation control");
    assert_eq!(
        expire_due_holds(&pool, 1100, 1)
            .await
            .expect("record invalid stock"),
        ExpiryBatch {
            expired: 0,
            failed: 1
        }
    );
    let (code,): (String,) =
        sqlx::query_as("SELECT code FROM stock_hold_expiry_failures WHERE hold_id=$1")
            .bind(id(1))
            .fetch_one(&pool)
            .await
            .expect("static code");
    assert_eq!(code, "invalid_stock");
    pool.execute("UPDATE stock_balances SET reserved=2 WHERE component_code='DEMO-COVER'")
        .await
        .expect("repair");
    transition_hold(
        &pool,
        &id(1),
        HoldCommand::Release {
            owner_id: "fixture-owner",
            actor_id: "fixture-actor",
        },
        1101,
    )
    .await
    .expect("owner release resolves failure");
    let (resolved,): (Option<i64>,) =
        sqlx::query_as("SELECT resolved_at FROM stock_hold_expiry_failures WHERE hold_id=$1")
            .bind(id(1))
            .fetch_one(&pool)
            .await
            .expect("resolved");
    assert_eq!(resolved, Some(1101));
    hold(&pool, 2, "DEMO-COVERS", 1100).await;
    pool.execute(
        "ALTER TABLE stock_holds DISABLE TRIGGER guard_hold_state;
        ALTER TABLE stock_holds DROP CONSTRAINT stock_hold_window;
        UPDATE stock_holds SET expires_at=999 WHERE id='00000000000000000000000000000002'",
    )
    .await
    .expect("hostile window");
    assert_eq!(
        expire_due_holds(&pool, 1100, 1)
            .await
            .expect("record policy error"),
        ExpiryBatch {
            expired: 0,
            failed: 1
        }
    );
    let (code,): (String,) =
        sqlx::query_as("SELECT code FROM stock_hold_expiry_failures WHERE hold_id=$1")
            .bind(id(2))
            .fetch_one(&pool)
            .await
            .expect("static code");
    assert_eq!(code, "invalid_hold");
    pool.execute(
        "UPDATE stock_holds SET expires_at=1100 WHERE id='00000000000000000000000000000002'",
    )
    .await
    .expect("repair");
    assert_eq!(
        expire_due_holds(&pool, 1160, 25).await.expect("recover"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_batch_inputs_and_unavailable_database_fail_closed(pool: PgPool) {
    seed(&pool).await;
    hold(&pool, 1, "DEMO-COVERS", 1100).await;
    for (now, limit) in [(-1, 1), (1100, 0), (1100, 101), (i64::MAX, 1)] {
        assert!(matches!(
            expire_due_holds(&pool, now, limit).await,
            Err(HoldWriteError::InvalidRequest)
        ));
    }
    assert_eq!(state(&pool, 1).await, "active");
    pool.close().await;
    assert!(matches!(
        expire_due_holds(&pool, 1100, 1).await,
        Err(HoldWriteError::Database(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelling_a_claim_rolls_back_all_effects_and_a_new_pass_recovers(pool: PgPool) {
    seed(&pool).await;
    hold(&pool, 1, "DEMO-FILLED", 1100).await;
    pool.execute("CREATE FUNCTION block_test_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='expire-hold' THEN PERFORM pg_advisory_xact_lock(710264); END IF; RETURN NEW; END; $$;
        CREATE TRIGGER block_test_expiry BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION block_test_expiry()")
        .await.expect("crash control");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(710264)")
        .execute(&mut *blocker)
        .await
        .expect("block effect");
    let worker_pool = pool.clone();
    let worker = tokio::spawn(async move { expire_due_holds(&worker_pool, 1100, 1).await });
    tokio::time::timeout(std::time::Duration::from_millis(800),async {
        loop {
            let (waiting,):(bool,)=sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory')").fetch_one(&pool).await.expect("lock probe");
            if waiting {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("worker reached blocked effect");
    worker.abort();
    assert!(worker.await.expect_err("cancelled worker").is_cancelled());
    blocker.rollback().await.expect("release control");
    pool.execute("DROP TRIGGER block_test_expiry ON stock_movements")
        .await
        .expect("remove control after rollback");
    assert_eq!(state(&pool, 1).await, "active");
    conserved(&pool).await;
    assert_eq!(
        expire_due_holds(&pool, 1100, 1).await.expect("restart"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    conserved(&pool).await;
}

#[sqlx::test(migrations = false)]
async fn expiry_upgrade_preserves_existing_hold_balances_and_old_reader(pool: PgPool) {
    MIGRATOR
        .run_to(5, &pool)
        .await
        .expect("previous hold schema");
    seed(&pool).await;
    hold(&pool, 1, "DEMO-FILLED", 1100).await;
    MIGRATOR.run(&pool).await.expect("expand retry metadata");
    assert_eq!(state(&pool, 1).await, "active");
    assert_eq!(
        expire_due_holds(&pool, 1100, 1)
            .await
            .expect("new expiry worker"),
        ExpiryBatch {
            expired: 1,
            failed: 0
        }
    );
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cooldown_committed_between_snapshot_and_claim_prevents_an_immediate_retry(pool: PgPool) {
    seed(&pool).await;
    hold(&pool, 1, "DEMO-COVERS", 1100).await;
    // A restricted role lets RLS pause the real candidate statement before its
    // row lock, without adding timing hooks to production worker code.
    pool.execute("CREATE ROLE expiry_overlap_role;
        GRANT USAGE ON SCHEMA public TO expiry_overlap_role;
        GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO expiry_overlap_role;
        GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO expiry_overlap_role;
        CREATE FUNCTION pause_expiry_snapshot() RETURNS boolean LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(917268); RETURN true; END; $$;
        ALTER TABLE stock_holds ENABLE ROW LEVEL SECURITY;
        CREATE POLICY pause_expiry_snapshot ON stock_holds FOR ALL TO expiry_overlap_role USING (pause_expiry_snapshot()) WITH CHECK (true)")
        .await.expect("isolated timing role");
    let restricted = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE expiry_overlap_role")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET default_transaction_isolation='repeatable read'")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("restricted pool");
    let mut blocker = pool.begin().await.expect("timing blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(917268)")
        .execute(&mut *blocker)
        .await
        .expect("pause candidate");
    let worker_pool = restricted.clone();
    let worker = tokio::spawn(async move { expire_due_holds(&worker_pool, 1100, 1).await });
    tokio::time::timeout(std::time::Duration::from_millis(800),async {
        loop {
            let (waiting,):(bool,)=sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory')")
                .fetch_one(&pool).await.expect("candidate snapshot reached");
            if waiting {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("paused before row claim");
    let mut failing_worker = pool.begin().await.expect("competing worker claim");
    sqlx::query("SELECT id FROM stock_holds WHERE id=$1 FOR UPDATE")
        .bind(id(1))
        .execute(&mut *failing_worker)
        .await
        .expect("claim before failure");
    sqlx::query("INSERT INTO stock_hold_expiry_failures (hold_id,attempts,last_attempt_at,next_attempt_at,code) VALUES ($1,1,1100,1160,'database_error')")
        .bind(id(1)).execute(&mut *failing_worker).await.expect("record cooldown");
    failing_worker
        .commit()
        .await
        .expect("newer failure metadata committed");
    blocker.rollback().await.expect("resume stale candidate");
    assert_eq!(
        worker.await.expect("worker").expect("cooldown skipped"),
        ExpiryBatch::default()
    );
    assert_eq!(state(&pool, 1).await, "active");
    let retry:(i32,i64,Option<i64>)=sqlx::query_as("SELECT attempts,next_attempt_at,resolved_at FROM stock_hold_expiry_failures WHERE hold_id=$1")
        .bind(id(1)).fetch_one(&pool).await.expect("untouched retry state");
    assert_eq!(retry, (1, 1160, None));
    conserved(&pool).await;
    restricted.close().await;
}
