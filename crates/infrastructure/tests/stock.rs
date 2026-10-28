use commerce_domain::inventory::{StockBalance, WarehouseChange, WarehouseChange::*};
use commerce_infrastructure::{
    MIGRATOR,
    stock::{StockWriteError, WarehouseRequest, warehouse_movement},
};
use sqlx::{Executor, PgPool};

const FIXTURE: &str = include_str!("../../../tests/fixtures/bom.sql");
fn request<'a>(
    effect: &'a str,
    component: &'a str,
    change: WarehouseChange,
) -> WarehouseRequest<'a> {
    WarehouseRequest {
        effect_key: effect,
        component_code: component,
        change,
        actor: "fixture-warehouse",
        reason: "isolated warehouse test",
    }
}
async fn balance(pool: &PgPool, component: &str) -> (i64, i64, i64) {
    sqlx::query_as("SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code=$1")
        .bind(component)
        .fetch_one(pool)
        .await
        .expect("balance")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_is_conserved_audited_and_replay_is_stable(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    let original = warehouse_movement(&pool, request("receipt", "DEMO-COVER", Receive(12)))
        .await
        .expect("receive");
    assert!(!original.replayed);
    assert_eq!(
        original.balance,
        StockBalance::new(12, 0, 0).expect("oracle")
    );
    for (effect, change, expected) in [
        ("inspect", Quarantine(5), (12, 5, 0)),
        ("release-inspection", ReleaseQuarantine(2), (12, 3, 0)),
        ("damage", DisposeQuarantine(1), (11, 2, 0)),
        ("count", Count(9), (9, 2, 0)),
    ] {
        warehouse_movement(&pool, request(effect, "DEMO-COVER", change))
            .await
            .expect("movement");
        assert_eq!(balance(&pool, "DEMO-COVER").await, expected);
    }
    let replay = warehouse_movement(&pool, request("receipt", "DEMO-COVER", Receive(12)))
        .await
        .expect("retry after other effects");
    assert!(replay.replayed);
    assert_eq!(
        replay.balance, original.balance,
        "receipt is the original result, not current stock"
    );
    assert_eq!(balance(&pool, "DEMO-COVER").await, (9, 2, 0));
    let ledger:(i64,i64,i64,i64)=sqlx::query_as("SELECT count(*),sum(on_hand_delta)::bigint,sum(quarantine_delta)::bigint,sum(reserved_delta)::bigint FROM stock_movements WHERE component_code='DEMO-COVER'")
        .fetch_one(&pool).await.expect("independent ledger oracle");
    assert_eq!(ledger, (5, 9, 2, 0));
    let audit:(String,String,bool)=sqlx::query_as("SELECT actor,reason,recorded_at IS NOT NULL FROM stock_movements WHERE effect_key='receipt'")
        .fetch_one(&pool).await.expect("audit");
    assert_eq!(
        audit,
        (
            "fixture-warehouse".into(),
            "isolated warehouse test".into(),
            true
        )
    );
    for invalid in [
        "UPDATE stock_movements SET reason='rewrite'",
        "DELETE FROM stock_movements",
        "UPDATE stock_balances SET on_hand=-1",
        "UPDATE stock_balances SET quarantine=10",
        "UPDATE stock_balances SET reserved=10",
    ] {
        assert!(
            pool.execute(invalid).await.is_err(),
            "accepted invalid SQL {invalid}"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_effects_and_conflicting_retries_leave_no_partial_writes(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    warehouse_movement(&pool, request("receive", "DEMO-COVER", Receive(4)))
        .await
        .expect("receive");
    for change in [
        Quarantine(5),
        ReleaseQuarantine(1),
        DisposeQuarantine(1),
        Receive(0),
        Count(-1),
    ] {
        assert!(matches!(
            warehouse_movement(&pool, request("invalid", "DEMO-COVER", change)).await,
            Err(StockWriteError::InvalidStock(_))
        ));
    }
    for changed in [
        request("receive", "DEMO-COVER", Receive(5)),
        request("receive", "DEMO-COVER", Quarantine(4)),
        request("receive", "DEMO-FOAM", Receive(4)),
        WarehouseRequest {
            actor: "other-actor",
            ..request("receive", "DEMO-COVER", Receive(4))
        },
        WarehouseRequest {
            reason: "other reason",
            ..request("receive", "DEMO-COVER", Receive(4))
        },
    ] {
        assert!(matches!(
            warehouse_movement(&pool, changed).await,
            Err(StockWriteError::IdempotencyConflict)
        ));
    }
    for invalid in [
        request(" ", "DEMO-COVER", Receive(1)),
        WarehouseRequest {
            actor: "",
            ..request("invalid", "DEMO-COVER", Receive(1))
        },
        WarehouseRequest {
            reason: "\n",
            ..request("invalid", "DEMO-COVER", Receive(1))
        },
        request("invalid", "", Receive(1)),
    ] {
        assert!(matches!(
            warehouse_movement(&pool, invalid).await,
            Err(StockWriteError::InvalidRequest)
        ));
    }
    for field in ["effect", "actor", "reason", "component"] {
        let too_long = "x".repeat(if field == "reason" { 501 } else { 129 });
        let mut invalid = request("invalid", "DEMO-COVER", Receive(1));
        match field {
            "effect" => invalid.effect_key = &too_long,
            "actor" => invalid.actor = &too_long,
            "reason" => invalid.reason = &too_long,
            _ => invalid.component_code = &too_long,
        }
        assert!(matches!(
            warehouse_movement(&pool, invalid).await,
            Err(StockWriteError::InvalidRequest)
        ));
    }
    assert!(matches!(
        warehouse_movement(&pool, request("missing", "DEMO-MISSING", Receive(1))).await,
        Err(StockWriteError::Database(_))
    ));
    assert_eq!(balance(&pool, "DEMO-COVER").await, (4, 0, 0));
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM stock_movements),(SELECT count(*) FROM stock_balances)",
    )
    .fetch_one(&pool)
    .await
    .expect("counts");
    assert_eq!(counts, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_receipts_and_same_effect_retries_apply_exactly_once(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            warehouse_movement(&pool, request("one-receipt", "DEMO-COVER", Receive(8))).await
        }));
    }
    let mut applied = 0;
    for task in tasks {
        if !task.await.expect("task").expect("retry").replayed {
            applied += 1;
        }
    }
    assert_eq!(applied, 1);
    assert_eq!(balance(&pool, "DEMO-COVER").await, (8, 0, 0));
    let first = pool.clone();
    let second = pool.clone();
    let quarantine = tokio::spawn(async move {
        warehouse_movement(&first, request("hold-a", "DEMO-COVER", Quarantine(6))).await
    });
    let competing = tokio::spawn(async move {
        warehouse_movement(&second, request("hold-b", "DEMO-COVER", Quarantine(6))).await
    });
    let results = [
        quarantine.await.expect("task"),
        competing.await.expect("task"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(balance(&pool, "DEMO-COVER").await, (8, 6, 0));
    let totals: (i64, i64) = sqlx::query_as(
        "SELECT sum(on_hand_delta)::bigint,sum(quarantine_delta)::bigint FROM stock_movements",
    )
    .fetch_one(&pool)
    .await
    .expect("ledger");
    assert_eq!(totals, (8, 6));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn insert_failure_rolls_back_balance_and_invalid_storage_fails_closed(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute("CREATE FUNCTION reject_test_movement() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'isolated rollback probe'; END; $$;
        CREATE TRIGGER reject_test_movement BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION reject_test_movement()")
        .await.expect("failure control");
    let error = warehouse_movement(&pool, request("failed", "DEMO-COVER", Receive(3)))
        .await
        .expect_err("forced DB failure");
    match error {
        StockWriteError::Database(error) => assert_eq!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("P0001")
        ),
        other => panic!("wrong error {other:?}"),
    }
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM stock_balances")
        .fetch_one(&pool)
        .await
        .expect("rollback count");
    assert_eq!(count.0, 0);
    pool.execute("DROP TRIGGER reject_test_movement ON stock_movements")
        .await
        .expect("remove failure control");
    warehouse_movement(&pool, request("valid", "DEMO-COVER", Receive(3)))
        .await
        .expect("recovery");
    pool.execute("ALTER TABLE stock_balances DROP CONSTRAINT stock_balances_on_hand_check;
        ALTER TABLE stock_balances DROP CONSTRAINT stock_balances_check; UPDATE stock_balances SET on_hand=-1")
        .await.expect("hostile stored balance");
    assert!(matches!(
        warehouse_movement(&pool, request("corrupt", "DEMO-COVER", Receive(1))).await,
        Err(StockWriteError::InvalidStock(_))
    ));
    pool.close().await;
    assert!(matches!(
        warehouse_movement(&pool, request("closed", "DEMO-COVER", Receive(1))).await,
        Err(StockWriteError::Database(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn distinct_components_cannot_commit_the_same_effect_key(pool: PgPool) {
    pool.execute(FIXTURE).await.expect("fixture");
    pool.execute("CREATE FUNCTION wait_test_movement() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(764391); RETURN NEW; END; $$;
        CREATE TRIGGER wait_test_movement BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION wait_test_movement()")
        .await.expect("contention control");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(764391)")
        .execute(&mut *blocker)
        .await
        .expect("block inserts");
    let mut tasks = Vec::new();
    for component in ["DEMO-COVER", "DEMO-FOAM"] {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            warehouse_movement(&pool, request("collision", component, Receive(3))).await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_millis(800),async {
        loop {
            let (waiting,):(i64,)=sqlx::query_as("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory'").fetch_one(&pool).await.expect("lock probe");
            if waiting==2 {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("both inserts contend");
    blocker.rollback().await.expect("release");
    let mut applied = 0;
    let mut rejected = 0;
    for task in tasks {
        match task.await.expect("task") {
            Ok(_) => applied += 1,
            Err(StockWriteError::IdempotencyConflict) => rejected += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((applied, rejected), (1, 1));
    let totals:(i64,i64,i64)=sqlx::query_as("SELECT count(*),sum(on_hand)::bigint,(SELECT count(*) FROM stock_movements) FROM stock_balances").fetch_one(&pool).await.expect("no partial second balance");
    assert_eq!(totals, (1, 3, 1));
}

#[sqlx::test(migrations = false)]
async fn stock_upgrade_preserves_boms_without_inventing_warehouse_quantities(pool: PgPool) {
    MIGRATOR
        .run_to(3, &pool)
        .await
        .expect("component catalogue schema");
    pool.execute(FIXTURE).await.expect("BOM fixture");
    MIGRATOR.run(&pool).await.expect("stock expansion");
    let (empty,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_balances")
        .fetch_one(&pool)
        .await
        .expect("no guessed stock");
    assert_eq!(empty, 0);
    let bom = commerce_infrastructure::bom::bill_of_materials(&pool, "DEMO-FILLED", 1)
        .await
        .expect("old reader")
        .expect("sealed BOM preserved");
    assert_eq!(bom.components.len(), 2);
    warehouse_movement(&pool, request("after-upgrade", "DEMO-COVER", Receive(2)))
        .await
        .expect("new stock primitive");
    assert_eq!(balance(&pool, "DEMO-COVER").await, (2, 0, 0));
}
