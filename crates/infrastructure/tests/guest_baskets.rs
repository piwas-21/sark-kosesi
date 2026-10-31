use commerce_application::{
    guest_access::GuestLimits,
    request_context::{Clock, ContextError, Identifiers},
};
use commerce_domain::{guest_baskets::BasketUpdate, quotes::BasketLine};
use commerce_infrastructure::{MIGRATOR, guest_baskets::*};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tower_sessions::{Session, SessionStore};
struct MutableClock(AtomicU64);
impl Clock for MutableClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
struct Sequence(AtomicU64);
impl Identifiers for Sequence {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Ok(u128::from(self.0.fetch_add(1, Ordering::SeqCst) + 1).to_be_bytes())
    }
}
fn store(pool: &PgPool, limits: GuestLimits) -> (GuestSessionStore, Arc<MutableClock>) {
    let clock = Arc::new(MutableClock(AtomicU64::new(1100)));
    (
        GuestSessionStore::new(
            pool.clone(),
            clock.clone(),
            Arc::new(Sequence(AtomicU64::new(0))),
            limits,
        )
        .expect("valid fixture store"),
        clock,
    )
}
fn session(store: &GuestSessionStore) -> Session {
    Session::new(None, Arc::new(store.clone()), None)
}
fn change(key: &str, revision: i32, quantity: u32) -> BasketUpdate {
    BasketUpdate {
        expected_revision: revision,
        effect_key: key.into(),
        lines: vec![BasketLine {
            sku: "SET-A".into(),
            quantity,
        }],
    }
}
async fn issued(store: &GuestSessionStore) -> (Session, BasketView) {
    let s = session(store);
    let view = ensure_session(store, &s)
        .await
        .expect("issue anonymous basket");
    (s, view)
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn maintained_sessions_store_verifiers_and_no_market_staff_or_stock_authority(pool: PgPool) {
    let (store, _) = store(&pool, GuestLimits::default());
    let (s, view) = issued(&store).await;
    assert_eq!((view.revision, view.lines.len()), (0, 0));
    assert_eq!(
        view.csrf_token(),
        "0000000000000000000000000000000200000000000000000000000000000003"
    );
    let digest: String = sqlx::query_scalar("SELECT encode(token_hash,'hex') FROM guest_sessions")
        .fetch_one(&pool)
        .await
        .expect("stored verifier");
    assert_eq!(
        digest,
        "6fa37c7c850df56b65888127688062054c03bf590168930eb1c42591555f62c9"
    );
    let (created, expiry, size): (i64, i64, i32) =
        sqlx::query_as("SELECT created_at,expires_at,octet_length(token_hash) FROM guest_sessions")
            .fetch_one(&pool)
            .await
            .expect("bounded credential");
    assert_eq!((created, expiry, size), (1100, 605900, 32));
    let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM staff_accounts),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM catalogue_publication_revisions)").fetch_one(&pool).await.expect("no authority or commerce");
    assert_eq!(counts, (0, 0, 0, 0));
    let reused = ensure_session(&store, &s)
        .await
        .expect("reuse cookie owner");
    assert_eq!(reused.csrf_token(), view.csrf_token());
    let audit: Vec<String> =
        sqlx::query_scalar("SELECT action FROM guest_session_events ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("audit");
    assert_eq!(audit, vec!["issued"]);
    assert!(!format!("{store:?}").contains(view.csrf_token()));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn two_guests_cannot_change_each_others_baskets_or_supply_money_roles_and_owner_fields(
    pool: PgPool,
) {
    let (store, _) = store(&pool, GuestLimits::default());
    let (a, av) = issued(&store).await;
    let (b, bv) = issued(&store).await;
    let ak = GuestCredential::from_session(&a).expect("A");
    let bk = GuestCredential::from_session(&b).expect("B");
    assert!(matches!(
        write_basket(&store, &ak, bv.csrf_token(), &change("cross", 0, 1)).await,
        Err(GuestError::Csrf)
    ));
    assert_eq!(
        write_basket(&store, &ak, av.csrf_token(), &change("first", 0, 2))
            .await
            .expect("owned write"),
        BasketReceipt {
            revision: 1,
            replayed: false
        }
    );
    assert!(
        read_basket(&store, &bk)
            .await
            .expect("B empty")
            .lines
            .is_empty()
    );
    assert_eq!(
        read_basket(&store, &ak).await.expect("A").lines,
        vec![BasketLine {
            sku: "SET-A".into(),
            quantity: 2
        }]
    );
    let raw =
        r#"{"expected_revision":0,"effect_key":"one","lines":[{"sku":"SET-A","quantity":1}]}"#;
    for field in [
        "owner_id",
        "basket_id",
        "total",
        "role",
        "stock",
        "policy",
        "csrf_token",
    ] {
        let input = raw.replacen('{', &format!("{{\"{field}\":true,"), 1);
        assert!(serde_json::from_str::<BasketUpdate>(&input).is_err());
    }
    assert!(
        serde_json::from_str::<BasketUpdate>(
            &raw.replace("\"quantity\":1", "\"quantity\":1,\"price\":1")
        )
        .is_err()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn full_intent_retries_return_old_receipts_without_reverting_newer_basket(pool: PgPool) {
    let (store, _) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    let k = GuestCredential::from_session(&s).expect("key");
    write_basket(&store, &k, v.csrf_token(), &change("one", 0, 1))
        .await
        .expect("first");
    let empty = BasketUpdate {
        expected_revision: 1,
        effect_key: "empty".into(),
        lines: vec![],
    };
    write_basket(&store, &k, v.csrf_token(), &empty)
        .await
        .expect("empty basket");
    assert_eq!(
        write_basket(&store, &k, v.csrf_token(), &change("one", 0, 1))
            .await
            .expect("old receipt"),
        BasketReceipt {
            revision: 1,
            replayed: true
        }
    );
    let current = read_basket(&store, &k).await.expect("current");
    assert_eq!((current.revision, current.lines.len()), (2, 0));
    assert!(matches!(
        write_basket(&store, &k, v.csrf_token(), &change("one", 0, 2)).await,
        Err(GuestError::Conflict)
    ));
    assert!(matches!(
        write_basket(&store, &k, v.csrf_token(), &change("stale", 0, 1)).await,
        Err(GuestError::Conflict)
    ));
    let mut multi = change("multi", 2, 1);
    multi.lines.push(BasketLine {
        sku: "AAA".into(),
        quantity: 2,
    });
    write_basket(&store, &k, v.csrf_token(), &multi)
        .await
        .expect("canonical source");
    multi.lines.reverse();
    assert!(
        write_basket(&store, &k, v.csrf_token(), &multi)
            .await
            .expect("order-independent retry")
            .replayed
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn issuance_and_mutation_ceilings_are_shared_bounded_and_cannot_reset_on_clock_rollback(
    pool: PgPool,
) {
    let limits = GuestLimits {
        issue_per_minute: 2,
        mutations_per_minute: 2,
    };
    let (store, clock) = store(&pool, limits);
    let (a, av) = issued(&store).await;
    let _ = issued(&store).await;
    assert!(matches!(
        ensure_session(&store, &session(&store)).await,
        Err(GuestError::RateLimited)
    ));
    let ak = GuestCredential::from_session(&a).expect("key");
    write_basket(&store, &ak, av.csrf_token(), &change("a", 0, 1))
        .await
        .expect("one");
    write_basket(&store, &ak, av.csrf_token(), &change("b", 1, 2))
        .await
        .expect("two");
    assert!(matches!(
        write_basket(&store, &ak, av.csrf_token(), &change("c", 2, 3)).await,
        Err(GuestError::RateLimited)
    ));
    assert!(
        write_basket(&store, &ak, av.csrf_token(), &change("a", 0, 1))
            .await
            .expect("same effect does not consume quota")
            .replayed
    );
    clock.0.store(1160, Ordering::SeqCst);
    let _ = issued(&store).await;
    write_basket(&store, &ak, av.csrf_token(), &change("c", 2, 3))
        .await
        .expect("next window");
    clock.0.store(1100, Ordering::SeqCst);
    assert!(matches!(
        read_basket(&store, &ak).await,
        Err(GuestError::ClockUnavailable)
    ));
    assert!(ensure_session(&store, &session(&store)).await.is_err());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn revocation_and_expiry_deny_reads_writes_replays_and_cached_framework_saves(pool: PgPool) {
    let (store, clock) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    let id = s.id().expect("issued");
    let k = GuestCredential::from_session(&s).expect("key");
    write_basket(&store, &k, v.csrf_token(), &change("first", 0, 1))
        .await
        .expect("first");
    let cached = store.load(&id).await.expect("load").expect("live record");
    revoke_session(&store, &s, v.csrf_token())
        .await
        .expect("revoke");
    assert!(matches!(
        read_basket(&store, &k).await,
        Err(GuestError::Denied)
    ));
    assert!(matches!(
        write_basket(&store, &k, v.csrf_token(), &change("first", 0, 1)).await,
        Err(GuestError::Denied)
    ));
    assert!(store.save(&cached).await.is_err());
    assert!(store.load(&id).await.expect("no live record").is_none());
    let (fresh, fv) = issued(&store).await;
    let fresh_key = GuestCredential::from_session(&fresh).expect("fresh");
    clock.0.store(605900, Ordering::SeqCst);
    assert!(matches!(
        read_basket(&store, &fresh_key).await,
        Err(GuestError::Denied)
    ));
    assert!(
        write_basket(
            &store,
            &fresh_key,
            fv.csrf_token(),
            &change("expired", 0, 1)
        )
        .await
        .is_err()
    );
    let audit: Vec<String> =
        sqlx::query_scalar("SELECT action FROM guest_session_events ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("audit");
    assert_eq!(audit, vec!["issued", "revoked", "issued"]);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_incomplete_receipts_rewrites_and_forged_basket_head_cannot_commit(pool: PgPool) {
    let (store, _) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    let k = GuestCredential::from_session(&s).expect("key");
    for sql in [
        "UPDATE guest_sessions SET expires_at=expires_at+1",
        "UPDATE guest_sessions SET basket_id='ffffffffffffffffffffffffffffffff'",
        "UPDATE guest_baskets SET revision=1,last_effect_key='missing',lines='[]'",
        "INSERT INTO guest_basket_effects(basket_id,effect_key,expected_revision,revision,lines) SELECT id,'unapplied',0,1,'[]' FROM guest_baskets",
    ] {
        assert!(pool.execute(sql).await.is_err(), "reject {sql}");
    }
    write_basket(&store, &k, v.csrf_token(), &change("first", 0, 1))
        .await
        .expect("valid receipt");
    for sql in [
        "UPDATE guest_basket_effects SET lines='[]'",
        "DELETE FROM guest_basket_effects",
        "UPDATE guest_baskets SET lines='[]'",
        "UPDATE guest_baskets SET id='ffffffffffffffffffffffffffffffff'",
        "UPDATE guest_session_events SET action='revoked'",
        "DELETE FROM guest_session_events",
    ] {
        assert!(pool.execute(sql).await.is_err(), "reject {sql}");
    }
    assert_eq!(
        read_basket(&store, &k).await.expect("unchanged").revision,
        1
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_writers_have_one_version_and_exact_retry_has_one_effect(pool: PgPool) {
    let (store, _) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    let a = GuestCredential::from_session(&s).expect("a");
    let b = GuestCredential::from_session(&s).expect("b");
    let ra = change("a", 0, 1);
    let rb = change("b", 0, 2);
    let (x, y) = tokio::join!(
        write_basket(&store, &a, v.csrf_token(), &ra),
        write_basket(&store, &b, v.csrf_token(), &rb)
    );
    assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1);
    let ra = change("same", 1, 3);
    let (x, y) = tokio::join!(
        write_basket(&store, &a, v.csrf_token(), &ra),
        write_basket(&store, &b, v.csrf_token(), &ra)
    );
    assert_ne!(x.expect("first").replayed, y.expect("second").replayed);
}
async fn waiting(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(2),async{
  loop{let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("lock observer");if exists{return;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
 }).await.expect("operation reached real lock");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn waiting_basket_operations_recheck_expiry_and_credential_revocation_serializes(
    pool: PgPool,
) {
    let (store, clock) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    let mut blocker = pool.begin().await.expect("basket blocker");
    sqlx::query("SELECT id FROM guest_baskets FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("held basket");
    let (ws, key, csrf) = (
        store.clone(),
        GuestCredential::from_session(&s).expect("key"),
        v.csrf_token().to_owned(),
    );
    let write =
        tokio::spawn(async move { write_basket(&ws, &key, &csrf, &change("wait", 0, 1)).await });
    waiting(&pool, "SELECT id FROM guest_baskets WHERE id=%").await;
    clock.0.store(605900, Ordering::SeqCst);
    blocker.rollback().await.expect("resume");
    assert!(matches!(
        write.await.expect("task"),
        Err(GuestError::Denied)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_basket_effects")
            .fetch_one(&pool)
            .await
            .expect("rolled back"),
        0
    );
    clock.0.store(1100, Ordering::SeqCst);
    let mut blocker = pool.begin().await.expect("basket blocker");
    sqlx::query("SELECT id FROM guest_baskets FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("held basket");
    let (ws, key, csrf) = (
        store.clone(),
        GuestCredential::from_session(&s).expect("key"),
        v.csrf_token().to_owned(),
    );
    let write =
        tokio::spawn(
            async move { write_basket(&ws, &key, &csrf, &change("serialized", 0, 1)).await },
        );
    waiting(&pool, "SELECT id FROM guest_baskets WHERE id=%").await;
    let (rs, csrf) = (store.clone(), v.csrf_token().to_owned());
    let revoke = tokio::spawn(async move { revoke_session(&rs, &s, &csrf).await });
    waiting(&pool, "SELECT left(basket_id%FOR UPDATE").await;
    blocker.rollback().await.expect("resume");
    assert!(write.await.expect("write task").is_ok());
    assert!(revoke.await.expect("revoke task").is_ok());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_sessions")
            .fetch_one(&pool)
            .await
            .expect("revoked"),
        0
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_clock_after_actual_effect_wait_and_cancellation_leave_no_partial_basket(
    pool: PgPool,
) {
    let (store, clock) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    pool.execute("CREATE FUNCTION pause_guest_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.revision>OLD.revision THEN PERFORM pg_advisory_xact_lock(871233); END IF; RETURN NEW; END; $$; CREATE TRIGGER pause_guest_effect BEFORE UPDATE ON guest_baskets FOR EACH ROW EXECUTE FUNCTION pause_guest_effect()").await.expect("timing instrument");
    for mode in ["cancel", "expired", "positive"] {
        clock.0.store(1100, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("gate");
        sqlx::query("SELECT pg_advisory_xact_lock(871233)")
            .execute(&mut *blocker)
            .await
            .expect("hold");
        let (ws, key, csrf) = (
            store.clone(),
            GuestCredential::from_session(&s).expect("key"),
            v.csrf_token().to_owned(),
        );
        let work =
            tokio::spawn(async move { write_basket(&ws, &key, &csrf, &change(mode, 0, 1)).await });
        waiting(&pool, "UPDATE guest_baskets SET revision=$2%").await;
        if mode == "cancel" {
            work.abort();
            assert!(work.await.expect_err("cancelled").is_cancelled());
            blocker.rollback().await.expect("unlock");
        } else {
            if mode == "expired" {
                clock.0.store(605900, Ordering::SeqCst);
            }
            blocker.rollback().await.expect("unlock");
            let result = work.await.expect("task");
            if mode == "expired" {
                assert!(matches!(result, Err(GuestError::Denied)));
            } else {
                assert_eq!(result.expect("positive control").revision, 1);
            }
        }
        let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_basket_effects")
            .fetch_one(&pool)
            .await
            .expect("committed effects");
        assert_eq!(count, i64::from(mode == "positive"));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn maintained_store_rejects_forged_cached_identity_expiry_and_principal_collisions(
    pool: PgPool,
) {
    let (store, clock) = store(&pool, GuestLimits::default());
    let (s, v) = issued(&store).await;
    let id = s.id().expect("id");
    let record = store.load(&id).await.expect("load").expect("record");
    for (field, value) in [
        ("csrf_token", serde_json::json!("f".repeat(64))),
        ("basket_id", serde_json::json!("f".repeat(32))),
        ("created_at", serde_json::json!(-1)),
        ("expires_at", serde_json::json!(605901)),
        ("role", serde_json::json!("admin")),
    ] {
        let mut bad = record.clone();
        bad.data.get_mut("guest").expect("guest")[field] = value;
        assert!(store.save(&bad).await.is_err(), "{field}");
    }
    let mut bad = record.clone();
    bad.data.insert("extra".into(), serde_json::json!(true));
    assert!(store.save(&bad).await.is_err());
    let mut bad = record.clone();
    bad.data.clear();
    assert!(store.save(&bad).await.is_err());
    let mut bad = record.clone();
    bad.expiry_date += tower_sessions::cookie::time::Duration::seconds(1);
    assert!(store.save(&bad).await.is_err());
    store.save(&record).await.expect("valid save");
    let collision = GuestSessionStore::new(
        pool.clone(),
        clock.clone(),
        Arc::new(Sequence(AtomicU64::new(0))),
        GuestLimits::default(),
    )
    .expect("collision fixture");
    assert!(
        ensure_session(&collision, &session(&collision))
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_sessions")
            .fetch_one(&pool)
            .await
            .expect("one owner"),
        1
    );
    assert_eq!(
        ensure_session(&store, &s)
            .await
            .expect("retained")
            .csrf_token(),
        v.csrf_token()
    );
    clock.0.store(605900, Ordering::SeqCst);
    assert!(store.load(&id).await.expect("expired").is_none());
    assert!(store.save(&record).await.is_err());
}
struct BrokenIds;
impl Identifiers for BrokenIds {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Err(ContextError::EntropyUnavailable)
    }
}
struct BrokenClock;
impl Clock for BrokenClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Err(ContextError::ClockUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn entropy_time_and_invalid_configuration_fail_without_minting_credentials(pool: PgPool) {
    let good_clock = Arc::new(MutableClock(AtomicU64::new(1100)));
    let ids = Arc::new(Sequence(AtomicU64::new(0)));
    for limits in [
        GuestLimits {
            issue_per_minute: 0,
            mutations_per_minute: 1,
        },
        GuestLimits {
            issue_per_minute: 1,
            mutations_per_minute: 1001,
        },
    ] {
        assert!(
            GuestSessionStore::new(pool.clone(), good_clock.clone(), ids.clone(), limits).is_err()
        );
    }
    let broken = GuestSessionStore::new(
        pool.clone(),
        good_clock.clone(),
        Arc::new(BrokenIds),
        GuestLimits::default(),
    )
    .expect("store");
    assert!(matches!(
        ensure_session(&broken, &session(&broken)).await,
        Err(GuestError::EntropyUnavailable)
    ));
    let broken = GuestSessionStore::new(
        pool.clone(),
        Arc::new(BrokenClock),
        ids.clone(),
        GuestLimits::default(),
    )
    .expect("store");
    assert!(matches!(
        ensure_session(&broken, &session(&broken)).await,
        Err(GuestError::ClockUnavailable)
    ));
    for value in [u64::MAX, i64::MAX as u64] {
        good_clock.0.store(value, Ordering::SeqCst);
        let store = GuestSessionStore::new(
            pool.clone(),
            good_clock.clone(),
            ids.clone(),
            GuestLimits::default(),
        )
        .expect("store");
        assert!(matches!(
            ensure_session(&store, &session(&store)).await,
            Err(GuestError::ClockUnavailable)
        ));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_sessions")
            .fetch_one(&pool)
            .await
            .expect("no authority"),
        0
    );
}
