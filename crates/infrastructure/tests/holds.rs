use commerce_domain::holds::{HoldError, HoldState};
use commerce_domain::inventory::WarehouseChange;
use commerce_infrastructure::{
    MIGRATOR,
    holds::{HoldCommand, HoldRequest, HoldWriteError, reserve_bom, transition_hold},
    stock::{WarehouseRequest, warehouse_movement},
};
use sqlx::{Executor, PgPool};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "11111111111111111111111111111111";
fn request<'a>(id: &'a str, sku: &'a str, units: u16) -> HoldRequest<'a> {
    HoldRequest {
        id,
        owner_id: "fixture-owner",
        sku,
        revision: 1,
        units,
        created_at: 1000,
        expires_at: 1100,
    }
}
fn release(owner: &str) -> HoldCommand<'_> {
    HoldCommand::Release {
        owner_id: owner,
        actor_id: "fixture-server-actor",
    }
}
fn consume(owner: &str) -> HoldCommand<'_> {
    HoldCommand::Consume {
        owner_id: owner,
        actor_id: "fixture-server-actor",
    }
}
fn expiry() -> HoldCommand<'static> {
    HoldCommand::Expire {
        actor_id: "fixture-expiry-worker",
    }
}
async fn receive(pool: &PgPool, key: &str, component: &str, quantity: u32) {
    warehouse_movement(
        pool,
        WarehouseRequest {
            effect_key: key,
            component_code: component,
            change: WarehouseChange::Receive(quantity),
            actor: "fixture-warehouse",
            reason: "isolated hold fixture",
        },
    )
    .await
    .expect("receive");
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("fixture");
    receive(pool, "covers", "DEMO-COVER", 10).await;
    receive(pool, "foam", "DEMO-FOAM", 2).await;
}
async fn balances(pool: &PgPool) -> Vec<(String, i64, i64, i64)> {
    sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code")
        .fetch_all(pool).await.expect("balances")
}
async fn conserved(pool: &PgPool) {
    let (mismatches,):(i64,)=sqlx::query_as("SELECT count(*) FROM stock_balances b WHERE (b.on_hand,b.quarantine,b.reserved) IS DISTINCT FROM (SELECT ROW(sum(on_hand_delta)::bigint,sum(quarantine_delta)::bigint,sum(reserved_delta)::bigint) FROM stock_movements m WHERE m.component_code=b.component_code)")
        .fetch_one(pool).await.expect("independent conservation oracle");
    assert_eq!(mismatches, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn covers_only_never_reserves_foam_and_snapshots_exact_bom_quantity(pool: PgPool) {
    seed(&pool).await;
    let first = reserve_bom(&pool, request(A, "DEMO-COVERS", 2))
        .await
        .expect("covers");
    assert_eq!(first.state, HoldState::Active);
    assert!(!first.replayed);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 10, 0, 4),
            ("DEMO-FOAM".into(), 2, 0, 0)
        ]
    );
    let retry = reserve_bom(&pool, request(A, "DEMO-COVERS", 2))
        .await
        .expect("replay");
    assert!(retry.replayed);
    reserve_bom(&pool, request(B, "DEMO-FILLED", 1))
        .await
        .expect("filled");
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 10, 0, 6),
            ("DEMO-FOAM".into(), 2, 0, 2)
        ]
    );
    let lines:Vec<(String,i64)>=sqlx::query_as("SELECT component_code,quantity FROM stock_hold_lines WHERE hold_id=$1 ORDER BY component_code").bind(A).fetch_all(&pool).await.expect("snapshot");
    assert_eq!(lines, vec![("DEMO-COVER".into(), 4)]);
    for invalid in [
        "UPDATE stock_hold_lines SET quantity=1",
        "DELETE FROM stock_hold_lines",
        "UPDATE stock_holds SET owner_id='intruder'",
        "UPDATE stock_holds SET expires_at=1200",
        "DELETE FROM stock_holds",
    ] {
        assert!(pool.execute(invalid).await.is_err(), "accepted {invalid}");
    }
    assert!(
        sqlx::query("INSERT INTO stock_hold_lines VALUES ($1,'DEMO-FOAM',1)")
            .bind(A)
            .execute(&pool)
            .await
            .is_err()
    );
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn last_shared_foam_is_atomic_and_failed_partial_bom_reservation_rolls_back(pool: PgPool) {
    seed(&pool).await;
    let left = pool.clone();
    let right = pool.clone();
    let first = tokio::spawn(async move { reserve_bom(&left, request(A, "DEMO-FILLED", 1)).await });
    let second =
        tokio::spawn(async move { reserve_bom(&right, request(B, "DEMO-FILLED", 1)).await });
    let results = [first.await.expect("task"), second.await.expect("task")];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(HoldWriteError::Stock(_))))
            .count(),
        1
    );
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 10, 0, 2),
            ("DEMO-FOAM".into(), 2, 0, 2)
        ]
    );
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("atomic snapshot");
    assert_eq!(count, 1);
    assert!(matches!(
        reserve_bom(&pool, request(C, "DEMO-FILLED", 1)).await,
        Err(HoldWriteError::Stock(_))
    ));
    assert_eq!(
        balances(&pool).await[0].3,
        2,
        "no leaked cover hold after foam scarcity"
    );
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn ownership_expiry_release_and_consumption_are_idempotent_and_distinct(pool: PgPool) {
    seed(&pool).await;
    reserve_bom(&pool, request(A, "DEMO-FILLED", 1))
        .await
        .expect("reserve");
    assert!(matches!(
        transition_hold(&pool, A, release("other-owner"), 1050).await,
        Err(HoldWriteError::Ownership)
    ));
    assert!(matches!(
        transition_hold(&pool, A, consume("other-owner"), 1050).await,
        Err(HoldWriteError::Ownership)
    ));
    assert!(matches!(
        transition_hold(&pool, A, expiry(), 1099).await,
        Err(HoldWriteError::Policy(HoldError::NotExpired))
    ));
    assert!(matches!(
        transition_hold(&pool, A, consume("fixture-owner"), 1100).await,
        Err(HoldWriteError::Policy(HoldError::Expired))
    ));
    let expired = transition_hold(&pool, A, expiry(), 1100)
        .await
        .expect("expiry");
    assert_eq!(expired.state, HoldState::Expired);
    assert!(!expired.replayed);
    assert!(
        transition_hold(&pool, A, expiry(), 1101)
            .await
            .expect("expiry retry")
            .replayed
    );
    assert!(matches!(
        transition_hold(&pool, A, consume("fixture-owner"), 1101).await,
        Err(HoldWriteError::Policy(HoldError::InvalidTransition))
    ));
    reserve_bom(&pool, request(B, "DEMO-FILLED", 1))
        .await
        .expect("reallocation is explicit");
    let consumed = transition_hold(&pool, B, consume("fixture-owner"), 1099)
        .await
        .expect("consume");
    assert_eq!(consumed.state, HoldState::Consumed);
    assert!(
        transition_hold(&pool, B, consume("fixture-owner"), 1101)
            .await
            .expect("consume retry")
            .replayed
    );
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 8, 0, 0),
            ("DEMO-FOAM".into(), 0, 0, 0)
        ]
    );
    reserve_bom(&pool, request(C, "DEMO-COVERS", 1))
        .await
        .expect("covers remain");
    assert_eq!(
        transition_hold(&pool, C, release("fixture-owner"), 1050)
            .await
            .expect("release")
            .state,
        HoldState::Released
    );
    assert!(
        transition_hold(&pool, C, release("fixture-owner"), 1051)
            .await
            .expect("release retry")
            .replayed
    );
    assert!(matches!(
        transition_hold(&pool, "missing", expiry(), 1100).await,
        Err(HoldWriteError::NotFound)
    ));
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_retries_and_expiry_release_never_duplicate_effects(pool: PgPool) {
    seed(&pool).await;
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            reserve_bom(&pool, request(A, "DEMO-FILLED", 1)).await
        }));
    }
    let mut applied = 0;
    for task in tasks {
        if !task.await.expect("task").expect("retry").replayed {
            applied += 1;
        }
    }
    assert_eq!(applied, 1);
    let first = pool.clone();
    let second = pool.clone();
    let expire = tokio::spawn(async move { transition_hold(&first, A, expiry(), 1100).await });
    let release =
        tokio::spawn(
            async move { transition_hold(&second, A, release("fixture-owner"), 1100).await },
        );
    let results = [expire.await.expect("task"), release.await.expect("task")];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 10, 0, 0),
            ("DEMO-FOAM".into(), 2, 0, 0)
        ]
    );
    let (effects,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM stock_movements WHERE hold_id=$1")
            .bind(A)
            .fetch_one(&pool)
            .await
            .expect("unique effects");
    assert_eq!(effects, 4, "two reserve and two release effects");
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_requests_conflicting_identity_and_ledger_failure_fail_closed(pool: PgPool) {
    seed(&pool).await;
    let too_long = "x".repeat(129);
    for invalid in [
        HoldRequest {
            id: "short",
            ..request(A, "DEMO-COVERS", 1)
        },
        HoldRequest {
            id: "gggggggggggggggggggggggggggggggg",
            ..request(A, "DEMO-COVERS", 1)
        },
        HoldRequest {
            owner_id: " ",
            ..request(A, "DEMO-COVERS", 1)
        },
        HoldRequest {
            owner_id: &too_long,
            ..request(A, "DEMO-COVERS", 1)
        },
        request(A, "DEMO-COVERS", 0),
        request(A, "DEMO-COVERS", 1001),
        HoldRequest {
            revision: 0,
            ..request(A, "DEMO-COVERS", 1)
        },
        request(A, "", 1),
        request(A, &too_long, 1),
    ] {
        assert!(matches!(
            reserve_bom(&pool, invalid).await,
            Err(HoldWriteError::InvalidRequest)
        ));
    }
    assert!(matches!(
        reserve_bom(
            &pool,
            HoldRequest {
                expires_at: 999,
                ..request(A, "DEMO-COVERS", 1)
            }
        )
        .await,
        Err(HoldWriteError::Policy(_))
    ));
    assert!(matches!(
        reserve_bom(&pool, request(A, "DEMO-MISSING", 1)).await,
        Err(HoldWriteError::MissingBom)
    ));
    reserve_bom(&pool, request(A, "DEMO-COVERS", 1))
        .await
        .expect("valid");
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',1);
        UPDATE catalogue_boms SET sealed=true WHERE revision=2",
    )
    .await
    .expect("alternative sealed revision");
    for changed in [
        HoldRequest {
            owner_id: "different",
            ..request(A, "DEMO-COVERS", 1)
        },
        request(A, "DEMO-FILLED", 1),
        HoldRequest {
            revision: 2,
            ..request(A, "DEMO-COVERS", 1)
        },
        request(A, "DEMO-COVERS", 2),
        HoldRequest {
            created_at: 1001,
            ..request(A, "DEMO-COVERS", 1)
        },
        HoldRequest {
            expires_at: 1101,
            ..request(A, "DEMO-COVERS", 1)
        },
    ] {
        assert!(matches!(
            reserve_bom(&pool, changed).await,
            Err(HoldWriteError::IdempotencyConflict)
        ));
    }
    pool.execute("CREATE FUNCTION fail_hold_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='reserve' THEN RAISE EXCEPTION 'isolated failure'; END IF; RETURN NEW; END; $$;
        CREATE TRIGGER fail_hold_effect BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION fail_hold_effect()")
        .await.expect("rollback probe");
    assert!(matches!(
        reserve_bom(&pool, request(B, "DEMO-FILLED", 1)).await,
        Err(HoldWriteError::Database(_))
    ));
    assert_eq!(balances(&pool).await[0].3, 2);
    assert!(matches!(
        transition_hold(&pool, B, expiry(), 1100).await,
        Err(HoldWriteError::NotFound)
    ));
    conserved(&pool).await;
    pool.close().await;
    assert!(matches!(
        reserve_bom(&pool, request(C, "DEMO-COVERS", 1)).await,
        Err(HoldWriteError::Database(_))
    ));
    assert!(matches!(
        transition_hold(&pool, A, expiry(), 1100).await,
        Err(HoldWriteError::Database(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn incomplete_mismatched_and_draft_bom_snapshots_cannot_commit(pool: PgPool) {
    seed(&pool).await;
    for invalid in [
        "INSERT INTO stock_holds (id,owner_id,sku,revision,units,created_at,expires_at) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','owner','DEMO-COVERS',1,1,1000,1100)",
        "INSERT INTO stock_holds VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','owner','DEMO-COVERS',1,1,1000,1100,'active')",
        "INSERT INTO stock_holds VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','owner','DEMO-COVERS',1,0,1000,1100,'building')",
        "INSERT INTO stock_holds VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','owner','DEMO-COVERS',1,1,1000,1000,'building')",
        "INSERT INTO stock_holds VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','owner','DEMO-COVERS',1,1,-1,1000,'building')",
        "INSERT INTO stock_holds VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','owner','DEMO-COVERS',1,1,1000,2801,'building')",
        "INSERT INTO stock_holds VALUES ('AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA','owner','DEMO-COVERS',1,1,1000,1100,'building')",
    ] {
        assert!(pool.execute(invalid).await.is_err(), "accepted {invalid}");
    }
    for lines in [
        "",
        "INSERT INTO stock_hold_lines VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','DEMO-COVER',3);",
        "INSERT INTO stock_hold_lines VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','DEMO-COVER',2),('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','DEMO-FOAM',1);",
    ] {
        let mut tx = pool.begin().await.expect("mismatched snapshot transaction");
        sqlx::query("INSERT INTO stock_holds (id,owner_id,sku,revision,units,created_at,expires_at) VALUES ($1,'owner','DEMO-COVERS',1,1,1000,1100)")
            .bind(A).execute(&mut *tx).await.expect("building");
        if !lines.is_empty() {
            tx.execute(lines).await.expect("mismatched lines");
        }
        assert!(
            sqlx::query("UPDATE stock_holds SET state='active' WHERE id=$1")
                .bind(A)
                .execute(&mut *tx)
                .await
                .is_err(),
            "invalid snapshot must fail"
        );
        tx.rollback().await.expect("rollback mismatch");
    }
    pool.execute(
        "INSERT INTO catalogue_boms (sku,revision) VALUES ('DEMO-COVERS',2);
        INSERT INTO catalogue_bom_lines VALUES ('DEMO-COVERS',2,'DEMO-COVER',2)",
    )
    .await
    .expect("unsealed draft");
    let mut tx = pool.begin().await.expect("draft snapshot transaction");
    sqlx::query("INSERT INTO stock_holds (id,owner_id,sku,revision,units,created_at,expires_at) VALUES ($1,'owner','DEMO-COVERS',2,1,1000,1100)")
        .bind(A).execute(&mut *tx).await.expect("building draft");
    sqlx::query("INSERT INTO stock_hold_lines VALUES ($1,'DEMO-COVER',2)")
        .bind(A)
        .execute(&mut *tx)
        .await
        .expect("draft line");
    assert!(
        sqlx::query("UPDATE stock_holds SET state='active' WHERE id=$1")
            .bind(A)
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.expect("rollback draft");
    reserve_bom(&pool, request(A, "DEMO-COVERS", 1))
        .await
        .expect("positive control");
    transition_hold(&pool, A, consume("fixture-owner"), 1099)
        .await
        .expect("consume");
    for invalid in [
        "UPDATE stock_holds SET state='active'",
        "UPDATE stock_holds SET state='building'",
        "UPDATE stock_holds SET units=2",
    ] {
        assert!(
            pool.execute(invalid).await.is_err(),
            "history must not be rewritten"
        );
    }
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_hold_storage_and_missing_lines_fail_without_changing_stock(pool: PgPool) {
    seed(&pool).await;
    reserve_bom(&pool, request(A, "DEMO-COVERS", 1))
        .await
        .expect("reserve");
    pool.execute(
        "ALTER TABLE stock_holds DISABLE TRIGGER guard_hold_state;
        ALTER TABLE stock_holds DROP CONSTRAINT stock_holds_state_check;
        UPDATE stock_holds SET state='mystery'",
    )
    .await
    .expect("hostile state");
    assert!(matches!(
        transition_hold(&pool, A, expiry(), 1100).await,
        Err(HoldWriteError::Database(_))
    ));
    assert!(matches!(
        reserve_bom(&pool, request(A, "DEMO-COVERS", 1)).await,
        Err(HoldWriteError::Database(_))
    ));
    pool.execute(
        "ALTER TABLE stock_holds DROP CONSTRAINT stock_hold_window;
        UPDATE stock_holds SET state='active', expires_at=999",
    )
    .await
    .expect("hostile window");
    assert!(matches!(
        transition_hold(&pool, A, expiry(), 1100).await,
        Err(HoldWriteError::Policy(HoldError::InvalidWindow))
    ));
    pool.execute(
        "UPDATE stock_holds SET expires_at=1100;
        ALTER TABLE stock_hold_lines DISABLE TRIGGER guard_hold_line;
        DELETE FROM stock_hold_lines",
    )
    .await
    .expect("hostile empty snapshot");
    assert!(matches!(
        transition_hold(&pool, A, expiry(), 1100).await,
        Err(HoldWriteError::Database(_))
    ));
    assert_eq!(balances(&pool).await[0].3, 2);
    conserved(&pool).await;
}

#[sqlx::test(migrations = false)]
async fn hold_upgrade_preserves_warehouse_balances_and_movement_history(pool: PgPool) {
    MIGRATOR
        .run_to(4, &pool)
        .await
        .expect("previous stock schema");
    seed(&pool).await;
    MIGRATOR.run(&pool).await.expect("expand hold schema");
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 10, 0, 0),
            ("DEMO-FOAM".into(), 2, 0, 0)
        ]
    );
    reserve_bom(&pool, request(A, "DEMO-FILLED", 1))
        .await
        .expect("new primitive");
    receive(&pool, "old-writer", "DEMO-COVER", 2).await;
    assert_eq!(balances(&pool).await[0], ("DEMO-COVER".into(), 12, 0, 2));
    conserved(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn corrupt_balance_or_failed_balance_update_cannot_create_a_hold(pool: PgPool) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION reject_reserved_balance() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'isolated balance failure'; END; $$;
        CREATE TRIGGER reject_reserved_balance BEFORE UPDATE ON stock_balances FOR EACH ROW EXECUTE FUNCTION reject_reserved_balance()")
        .await.expect("balance write failure control");
    assert!(matches!(
        reserve_bom(&pool, request(A, "DEMO-FILLED", 1)).await,
        Err(HoldWriteError::Database(_))
    ));
    assert_eq!(
        balances(&pool).await,
        vec![
            ("DEMO-COVER".into(), 10, 0, 0),
            ("DEMO-FOAM".into(), 2, 0, 0)
        ]
    );
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("no partial hold");
    assert_eq!(count, 0);
    conserved(&pool).await;
    pool.execute(
        "DROP TRIGGER reject_reserved_balance ON stock_balances;
        ALTER TABLE stock_balances DROP CONSTRAINT stock_balances_on_hand_check;
        ALTER TABLE stock_balances DROP CONSTRAINT stock_balances_check;
        UPDATE stock_balances SET on_hand=-1 WHERE component_code='DEMO-COVER'",
    )
    .await
    .expect("hostile physical balance");
    assert!(matches!(
        reserve_bom(&pool, request(B, "DEMO-FILLED", 1)).await,
        Err(HoldWriteError::Stock(_))
    ));
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_holds")
        .fetch_one(&pool)
        .await
        .expect("no corrupt hold");
    assert_eq!(count, 0);
    assert_eq!(balances(&pool).await[0].3, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_records_the_worker_actor_and_rejects_empty_audit_identity(pool: PgPool) {
    seed(&pool).await;
    reserve_bom(&pool, request(A, "DEMO-COVERS", 1))
        .await
        .expect("reserve");
    let long_actor = "x".repeat(129);
    for actor_id in [" ", long_actor.as_str()] {
        assert!(matches!(
            transition_hold(&pool, A, HoldCommand::Expire { actor_id }, 1100).await,
            Err(HoldWriteError::InvalidRequest)
        ));
    }
    transition_hold(&pool, A, expiry(), 1100)
        .await
        .expect("worker expiry");
    let (actor,): (String,) = sqlx::query_as(
        "SELECT actor FROM stock_movements WHERE hold_id=$1 AND action='expire-hold'",
    )
    .bind(A)
    .fetch_one(&pool)
    .await
    .expect("audit actor");
    assert_eq!(
        actor, "fixture-expiry-worker",
        "worker effect must not be attributed to the customer owner"
    );
    conserved(&pool).await;
}
