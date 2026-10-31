use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::holds::HoldState;
use commerce_infrastructure as infrastructure;
use commerce_infrastructure::{MIGRATOR, holds::*};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
#[path = "support/basket_holds_fixture.rs"]
mod fixture;
use fixture::*;
const GROUP: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";
const OWNER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn clock() -> Time {
    Time(AtomicU64::new(1100))
}
fn lines(offset: u128) -> Vec<BasketHoldLine> {
    vec![
        BasketHoldLine {
            id: format!("{:032x}", offset + 2),
            sku: "SET-FILLED".into(),
            revision: 1,
            units: 1,
        },
        BasketHoldLine {
            id: format!("{:032x}", offset + 1),
            sku: "SET-COVERS".into(),
            revision: 1,
            units: 1,
        },
    ]
}
fn request<'a>(id: &'a str, lines: &'a [BasketHoldLine]) -> BasketHoldRequest<'a> {
    BasketHoldRequest {
        id,
        owner_id: OWNER,
        created_at: 1100,
        expires_at: 1200,
        lines,
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn one_commit_reserves_all_shared_components_and_replay_returns_canonical_full_manifest(
    pool: PgPool,
) {
    seed(&pool, 10, 4).await;
    let mut lines = lines(0);
    let first = reserve_basket(&pool, &clock(), request(GROUP, &lines))
        .await
        .expect("whole basket");
    assert!(!first.replayed);
    assert_eq!(
        first
            .holds
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        [
            "00000000000000000000000000000001",
            "00000000000000000000000000000002"
        ]
    );
    assert_eq!(
        balances(&pool).await,
        [
            ("SEAT-COVER".into(), 10, 0, 4),
            ("SEAT-FOAM".into(), 4, 0, 2)
        ]
    );
    assert_eq!(counts(&pool).await, (1, 2, 2, 3));
    lines.reverse();
    let replay = reserve_basket(&pool, &clock(), request(GROUP, &lines))
        .await
        .expect("permuted exact retry");
    assert!(replay.replayed);
    assert_eq!(
        replay
            .holds
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        first
            .holds
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(counts(&pool).await, (1, 2, 2, 3));
    conserved(&pool).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn last_shared_foam_has_one_whole_basket_winner_and_no_partial_loser(pool: PgPool) {
    seed(&pool, 8, 2).await;
    let a = lines(0);
    let b = lines(10);
    let time = clock();
    let (left, right) = tokio::join!(
        reserve_basket(&pool, &time, request(GROUP, &a)),
        reserve_basket(&pool, &time, request(OTHER, &b))
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(
        matches!(left, Err(HoldWriteError::Stock(_)))
            || matches!(right, Err(HoldWriteError::Stock(_)))
    );
    assert_eq!(
        balances(&pool).await,
        [
            ("SEAT-COVER".into(), 8, 0, 4),
            ("SEAT-FOAM".into(), 2, 0, 2)
        ]
    );
    assert_eq!(counts(&pool).await, (1, 2, 2, 3));
    conserved(&pool).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn scarcity_on_later_line_missing_bom_and_child_id_collision_rollback_every_effect(
    pool: PgPool,
) {
    seed(&pool, 10, 0).await;
    let lines = lines(0);
    assert!(matches!(
        reserve_basket(&pool, &clock(), request(GROUP, &lines)).await,
        Err(HoldWriteError::Stock(_))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    let mut missing = lines.clone();
    missing[0].revision = 2;
    assert!(matches!(
        reserve_basket(&pool, &clock(), request(GROUP, &missing)).await,
        Err(HoldWriteError::MissingBom)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    reserve_bom(
        &pool,
        HoldRequest {
            id: &lines[1].id,
            owner_id: OWNER,
            sku: "SET-COVERS",
            revision: 1,
            units: 1,
            created_at: 1100,
            expires_at: 1200,
        },
    )
    .await
    .expect("legacy standalone hold");
    let before = balances(&pool).await;
    assert!(
        reserve_basket(&pool, &clock(), request(GROUP, &lines))
            .await
            .is_err()
    );
    assert_eq!(counts(&pool).await, (0, 0, 1, 1));
    assert_eq!(balances(&pool).await, before);
    conserved(&pool).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_binds_owner_window_count_and_each_child_selection_field(pool: PgPool) {
    seed(&pool, 20, 10).await;
    let base = lines(0);
    reserve_basket(&pool, &clock(), request(GROUP, &base))
        .await
        .expect("original");
    for field in 0..9 {
        let mut selected = base.clone();
        let mut owner = OWNER;
        let mut created = 1100;
        let mut expiry = 1200;
        match field {
            0 => owner = OTHER,
            1 => created = 1099,
            2 => expiry = 1201,
            3 => {
                selected.pop();
            }
            4 => selected[0].id = "ffffffffffffffffffffffffffffffff".into(),
            5 => selected[0].sku = "SET-OTHER".into(),
            6 => selected[0].revision = 2,
            7 => selected[0].units = 2,
            8 => selected.push(BasketHoldLine {
                id: "ffffffffffffffffffffffffffffffff".into(),
                sku: "SET-OTHER".into(),
                revision: 1,
                units: 1,
            }),
            _ => unreachable!(),
        }
        let r = BasketHoldRequest {
            id: GROUP,
            owner_id: owner,
            created_at: created,
            expires_at: expiry,
            lines: &selected,
        };
        assert!(
            matches!(
                reserve_basket(&pool, &clock(), r).await,
                Err(HoldWriteError::IdempotencyConflict)
            ),
            "intent field {field}"
        );
    }
    assert_eq!(counts(&pool).await, (1, 2, 2, 3));
    conserved(&pool).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn same_group_concurrent_exact_retries_have_one_history_and_original_order(pool: PgPool) {
    seed(&pool, 10, 4).await;
    let selected = lines(0);
    let time = clock();
    let (a, b) = tokio::join!(
        reserve_basket(&pool, &time, request(GROUP, &selected)),
        reserve_basket(&pool, &time, request(GROUP, &selected))
    );
    assert_ne!(a.expect("first").replayed, b.expect("retry").replayed);
    assert_eq!(counts(&pool).await, (1, 2, 2, 3));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn covers_only_creates_no_foam_pool_and_worker_expiry_does_not_reactivate_history(
    pool: PgPool,
) {
    seed(&pool, 10, 0).await;
    let selected = vec![lines(0).remove(1)];
    reserve_basket(&pool, &clock(), request(GROUP, &selected))
        .await
        .expect("covers");
    assert_eq!(balances(&pool).await, [("SEAT-COVER".into(), 10, 0, 2)]);
    assert_eq!(
        expire_due_holds(&pool, 1200, 10)
            .await
            .expect("existing worker")
            .expired,
        1
    );
    let late = Time(AtomicU64::new(1201));
    let replay = reserve_basket(&pool, &late, request(GROUP, &selected))
        .await
        .expect("historical receipt only");
    assert!(replay.replayed);
    assert_eq!(replay.holds[0].state, HoldState::Expired);
    assert_eq!(balances(&pool).await, [("SEAT-COVER".into(), 10, 0, 0)]);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    conserved(&pool).await;
}
async fn waiting(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(900),async{loop{
  let reached:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("real lock observer");if reached{return;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;
 }}).await.expect("writer reached actual wait");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_wait_expiry_and_cancellation_rollback_manifest_children_and_balances(pool: PgPool) {
    seed(&pool, 10, 4).await;
    for mode in ["cancel", "expired", "rewind", "positive"] {
        let mut blocker = pool.begin().await.expect("stock blocker");
        sqlx::query("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
            .execute(&mut *blocker)
            .await
            .expect("stock held");
        let time = Arc::new(clock());
        let wc = time.clone();
        let wp = pool.clone();
        let selected = lines(0);
        let work = tokio::spawn(async move {
            reserve_basket(&wp, wc.as_ref(), request(GROUP, &selected)).await
        });
        waiting(
            &pool,
            "SELECT component_code,on_hand,quarantine,reserved FROM stock_balances%",
        )
        .await;
        if mode == "cancel" {
            work.abort();
            assert!(work.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("unlock");
        } else {
            if mode == "expired" {
                time.0.store(1200, Ordering::SeqCst);
            }
            if mode == "rewind" {
                time.0.store(1099, Ordering::SeqCst);
            }
            blocker.rollback().await.expect("unlock");
            let result = work.await.expect("task");
            match mode {
                "expired" => assert!(matches!(
                    result,
                    Err(HoldWriteError::Policy(
                        commerce_domain::holds::HoldError::Expired
                    ))
                )),
                "rewind" => assert!(matches!(result, Err(HoldWriteError::ClockUnavailable))),
                _ => assert!(result.is_ok()),
            }
        }
        if mode == "positive" {
            assert_eq!(counts(&pool).await, (1, 2, 2, 3));
        } else {
            assert_eq!(counts(&pool).await, (0, 0, 0, 0));
            assert_eq!(
                balances(&pool).await,
                [
                    ("SEAT-COVER".into(), 10, 0, 0),
                    ("SEAT-FOAM".into(), 4, 0, 0)
                ]
            );
        }
    }
    conserved(&pool).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_imports_cannot_rewrite_or_commit_incomplete_groups_or_adopt_legacy_holds(
    pool: PgPool,
) {
    seed(&pool, 20, 10).await;
    let selected = lines(0);
    reserve_basket(&pool, &clock(), request(GROUP, &selected))
        .await
        .expect("positive complete manifest");
    for sql in [
        "UPDATE basket_hold_groups SET owner_id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'",
        "UPDATE basket_hold_groups SET sealed=false",
        "DELETE FROM basket_hold_groups",
        "UPDATE basket_hold_members SET units=units",
        "DELETE FROM basket_hold_members",
        "UPDATE stock_holds SET basket_group_id=NULL WHERE basket_group_id IS NOT NULL",
        "INSERT INTO basket_hold_groups(id,owner_id,created_at,expires_at,members_count,sealed) VALUES('22222222222222222222222222222222','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',1100,1200,1,true)",
        "INSERT INTO basket_hold_groups(id,owner_id,created_at,expires_at,members_count) VALUES('22222222222222222222222222222222','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',1100,1200,1)",
        "INSERT INTO stock_holds(id,owner_id,sku,revision,units,created_at,expires_at,basket_group_id) VALUES('ffffffffffffffffffffffffffffffff','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','SET-COVERS',1,1,1100,1200,'11111111111111111111111111111111')",
    ] {
        let e = pool.execute(sql).await.expect_err("reject forged history");
        assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{sql}"
        );
    }
    let legacy = "ffffffffffffffffffffffffffffffff";
    reserve_bom(
        &pool,
        HoldRequest {
            id: legacy,
            owner_id: OWNER,
            sku: "SET-COVERS",
            revision: 1,
            units: 1,
            created_at: 1100,
            expires_at: 1200,
        },
    )
    .await
    .expect("standalone legacy positive control");
    for sql in [
        "UPDATE stock_holds SET basket_group_id='22222222222222222222222222222222' WHERE id='ffffffffffffffffffffffffffffffff'",
        "INSERT INTO basket_hold_members(group_id,hold_id,sku,revision,units) VALUES('22222222222222222222222222222222','ffffffffffffffffffffffffffffffff','SET-COVERS',1,1)",
    ] {
        let mut tx = pool.begin().await.expect("raw pending group");
        tx.execute("INSERT INTO basket_hold_groups(id,owner_id,created_at,expires_at,members_count) VALUES('22222222222222222222222222222222','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',1100,1200,1)").await.expect("open draft");
        let e = tx.execute(sql).await.expect_err("legacy adoption denied");
        assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
        tx.rollback().await.expect("discard import");
    }
    assert_eq!(counts(&pool).await, (1, 2, 3, 4));
    conserved(&pool).await;
}
struct BrokenClock;
impl Clock for BrokenClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Err(ContextError::ClockUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_envelopes_lifetimes_and_context_never_commit_stock_or_manifest(pool: PgPool) {
    seed(&pool, 20, 10).await;
    for case in 0..15 {
        let mut selected = lines(0);
        let mut id = GROUP;
        let mut owner = OWNER;
        let mut start = 1100;
        let mut expiry = 1200;
        match case {
            0 => id = "bad",
            1 => owner = "bad",
            2 => selected.clear(),
            3 => {
                selected = (0..21)
                    .map(|n| BasketHoldLine {
                        id: format!("{n:032x}"),
                        sku: format!("SKU-{n}"),
                        revision: 1,
                        units: 1,
                    })
                    .collect()
            }
            4 => selected[0].id = "bad".into(),
            5 => selected[0].sku = "DEMO-FILLED".into(),
            6 => selected[0].revision = 0,
            7 => selected[0].units = 0,
            8 => selected[0].units = 1001,
            9 => selected[0].sku = selected[1].sku.clone(),
            10 => selected[0].id = selected[1].id.clone(),
            11 => start = -1,
            12 => expiry = 1100,
            13 => expiry = 2901,
            14 => start = 1200,
            _ => unreachable!(),
        }
        let request = BasketHoldRequest {
            id,
            owner_id: owner,
            created_at: start,
            expires_at: expiry,
            lines: &selected,
        };
        assert!(
            reserve_basket(&pool, &clock(), request).await.is_err(),
            "envelope case {case}"
        );
        assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    }
    let selected = lines(0);
    assert!(matches!(
        reserve_basket(&pool, &BrokenClock, request(GROUP, &selected)).await,
        Err(HoldWriteError::ClockUnavailable)
    ));
    assert!(matches!(
        reserve_basket(
            &pool,
            &Time(AtomicU64::new(u64::MAX)),
            request(GROUP, &selected)
        )
        .await,
        Err(HoldWriteError::ClockUnavailable)
    ));
    assert!(matches!(
        reserve_basket(
            &pool,
            &Time(AtomicU64::new(1099)),
            request(GROUP, &selected)
        )
        .await,
        Err(HoldWriteError::Policy(
            commerce_domain::holds::HoldError::InvalidClock
        ))
    ));
    assert!(matches!(
        reserve_basket(
            &pool,
            &Time(AtomicU64::new(1200)),
            request(GROUP, &selected)
        )
        .await,
        Err(HoldWriteError::Policy(
            commerce_domain::holds::HoldError::Expired
        ))
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    conserved(&pool).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn global_component_claims_prevent_opposite_bom_order_deadlocks(pool: PgPool) {
    seed(&pool, 20, 0).await;
    receive(&pool, "OTHER-COVER", 2).await;
    for (sku, component) in [
        ("A-SEAT", "SEAT-COVER"),
        ("B-OTHER", "OTHER-COVER"),
        ("C-OTHER", "OTHER-COVER"),
        ("D-SEAT", "SEAT-COVER"),
    ] {
        variant(&pool, sku, false, &[(component, 1)]).await;
    }
    pool.execute("CREATE FUNCTION pause_first_basket_component() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.component_code='SEAT-COVER' AND OLD.reserved=0 AND NEW.reserved=1 THEN PERFORM pg_advisory_xact_lock(891233); END IF; RETURN NEW; END; $$; CREATE TRIGGER pause_first_basket_component BEFORE UPDATE ON stock_balances FOR EACH ROW EXECUTE FUNCTION pause_first_basket_component()").await.expect("timing instrument");
    let mut gate = pool.begin().await.expect("component gate");
    sqlx::query("SELECT pg_advisory_xact_lock(891233)")
        .execute(&mut *gate)
        .await
        .expect("pause first effect");
    let first_pool = pool.clone();
    let first = tokio::spawn(async move {
        let lines = vec![
            BasketHoldLine {
                id: "00000000000000000000000000000001".into(),
                sku: "A-SEAT".into(),
                revision: 1,
                units: 1,
            },
            BasketHoldLine {
                id: "00000000000000000000000000000002".into(),
                sku: "B-OTHER".into(),
                revision: 1,
                units: 1,
            },
        ];
        reserve_basket(&first_pool, &clock(), request(GROUP, &lines)).await
    });
    waiting(&pool, "UPDATE stock_balances SET on_hand=%").await;
    let named = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            pool.connect_options()
                .as_ref()
                .clone()
                .application_name("basket-order-b"),
        )
        .await
        .expect("second isolated database connection");
    let second = tokio::spawn(async move {
        let lines = vec![
            BasketHoldLine {
                id: "00000000000000000000000000000011".into(),
                sku: "C-OTHER".into(),
                revision: 1,
                units: 1,
            },
            BasketHoldLine {
                id: "00000000000000000000000000000012".into(),
                sku: "D-SEAT".into(),
                revision: 1,
                units: 1,
            },
        ];
        let receipt = reserve_basket(
            &named,
            &clock(),
            BasketHoldRequest {
                owner_id: OTHER,
                ..request(OTHER, &lines)
            },
        )
        .await;
        named.close().await;
        receipt
    });
    tokio::time::timeout(std::time::Duration::from_millis(900),async{loop{
  let reached:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='basket-order-b' AND wait_event_type='Lock')").fetch_one(&pool).await.expect("named lock observer");if reached{return;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;
 }}).await.expect("second order reached actual shared component wait");
    gate.rollback().await.expect("resume first order");
    let a = first.await.expect("first task");
    let b = second.await.expect("second task");
    assert!(
        a.is_ok() && b.is_ok(),
        "global component order must allow both complete baskets: {a:?}, {b:?}"
    );
    assert_eq!(counts(&pool).await, (2, 4, 4, 4));
    assert_eq!(
        balances(&pool).await,
        [
            ("OTHER-COVER".into(), 2, 0, 2),
            ("SEAT-COVER".into(), 20, 0, 2)
        ]
    );
    conserved(&pool).await;
}
struct ScriptClock(std::sync::Mutex<std::collections::VecDeque<u64>>);
impl Clock for ScriptClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
            .lock()
            .map_err(|_| ContextError::ClockUnavailable)?
            .pop_front()
            .ok_or(ContextError::ClockUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn admission_and_final_clocks_are_monotonic_and_failures_rollback_all_basket_effects(
    pool: PgPool,
) {
    seed(&pool, 10, 4).await;
    let selected = lines(0);
    for readings in [
        vec![1100, 1099],
        vec![1100, 1101, 1100],
        vec![1100, 1100, 1099],
        vec![1100],
        vec![1100, 1100],
        vec![1100, 1200],
        vec![1100, 1100, 1200],
    ] {
        let clock = ScriptClock(std::sync::Mutex::new(readings.into()));
        assert!(
            reserve_basket(&pool, &clock, request(GROUP, &selected))
                .await
                .is_err()
        );
        assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    }
    reserve_basket(&pool, &clock(), request(GROUP, &selected))
        .await
        .expect("positive after failed attempts");
    let reversed = ScriptClock(std::sync::Mutex::new(vec![1100, 1099].into()));
    assert!(matches!(
        reserve_basket(&pool, &reversed, request(GROUP, &selected)).await,
        Err(HoldWriteError::ClockUnavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 2, 2, 3));
    conserved(&pool).await;
}
#[sqlx::test(migrations = false)]
async fn migration_preserves_legacy_readers_standalone_hold_identity_and_stock(pool: PgPool) {
    MIGRATOR.run_to(17, &pool).await.expect("prior schema");
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("invented prior fixture");
    receive(&pool, "DEMO-COVER", 10).await;
    let prior = "ffffffffffffffffffffffffffffffff";
    reserve_bom(
        &pool,
        HoldRequest {
            id: prior,
            owner_id: "prior-owner",
            sku: "DEMO-COVERS",
            revision: 1,
            units: 1,
            created_at: 1100,
            expires_at: 1200,
        },
    )
    .await
    .expect("prior standalone hold");
    let before = balances(&pool).await;
    assert_eq!(before, [("DEMO-COVER".into(), 10, 0, 2)]);
    MIGRATOR
        .run(&pool)
        .await
        .expect("expand without modifying prior identity");
    let binding: Option<String> =
        sqlx::query_scalar("SELECT basket_group_id FROM stock_holds WHERE id=$1")
            .bind(prior)
            .fetch_one(&pool)
            .await
            .expect("legacy binding remains absent");
    assert_eq!(binding, None);
    assert_eq!(balances(&pool).await, before);
    assert_eq!(counts(&pool).await, (0, 0, 1, 1));
    let released = transition_hold(
        &pool,
        prior,
        HoldCommand::Release {
            owner_id: "prior-owner",
            actor_id: "fixture-warehouse",
        },
        1101,
    )
    .await
    .expect("old transition still works");
    assert_eq!(released.state, HoldState::Released);
    assert_eq!(balances(&pool).await, [("DEMO-COVER".into(), 10, 0, 0)]);
    conserved(&pool).await;
}
