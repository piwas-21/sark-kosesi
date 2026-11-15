//! Source failures must not escape after held guest custody expires.
use super::*;
use commerce_domain::quotes::QuoteError;
use commerce_infrastructure::quotes::QuoteFailure;
use std::sync::atomic::Ordering;

async fn wait_on(pool: &PgPool, fragment: &str) {
    for _ in 0..150 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND cardinality(pg_blocking_pids(pid))>0 AND query LIKE $1)",
        )
        .bind(format!("%{fragment}%"))
        .fetch_one(pool)
        .await
        .expect("inspect lock wait");
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(4)).await;
    }
    panic!("expected quote query did not wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn source_failure_after_wait_requires_current_guest_for_both_quote_paths(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let expires: i64 = sqlx::query_scalar("SELECT expires_at FROM guest_sessions")
        .fetch_one(&pool)
        .await
        .expect("guest expiry");
    for promoted in [false, true] {
        for stale in [false, true] {
            time.0.store(1100, Ordering::SeqCst);
            pool.execute("UPDATE stock_balances SET on_hand=100 WHERE component_code='SEAT-COVER'")
                .await
                .expect("reset stock");
            let mut blocker = pool.begin().await.expect("blocker");
            sqlx::query("UPDATE stock_balances SET on_hand=0 WHERE component_code='SEAT-COVER'")
                .execute(&mut *blocker)
                .await
                .expect("uncommitted scarcity");
            let key = GuestCredential::from_session(&session).expect("guest");
            let store = store.clone();
            let csrf = csrf.clone();
            let task = tokio::spawn(async move {
                if promoted {
                    issue_promoted_quote(
                        &store,
                        &key,
                        &csrf,
                        Locale::Nl,
                        &fixture::request("wait"),
                        "COURTYARD",
                    )
                    .await
                    .map(|_| ())
                } else {
                    issue_quote(&store, &key, &csrf, Locale::Nl, &fixture::request("wait"))
                        .await
                        .map(|_| ())
                }
            });
            wait_on(&pool, "SELECT component_code,on_hand,").await;
            if stale {
                time.0.store(expires as u64, Ordering::SeqCst);
            }
            blocker.commit().await.expect("expose scarcity");
            let result = task.await.expect("quote task");
            if stale {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::Denied))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Evaluation(QuoteFailure::Policy(
                        QuoteError::InsufficientStock
                    )))
                ));
            }
            assert_eq!(fixture::counts(&pool).await, (0, 0, 0));
            let quota: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_quote_quota")
                .fetch_one(&pool)
                .await
                .expect("rolled back quota");
            assert_eq!(quota, 0);
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn absent_quote_after_wait_requires_current_guest_for_both_read_paths(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (session, _) = fixture::guest(&store).await;
    let expires: i64 = sqlx::query_scalar("SELECT expires_at FROM guest_sessions")
        .fetch_one(&pool)
        .await
        .expect("guest expiry");
    for promoted in [false, true] {
        for stale in [false, true] {
            time.0.store(1100, Ordering::SeqCst);
            let mut blocker = pool.begin().await.expect("blocker");
            blocker
                .execute("LOCK TABLE guest_quote_snapshots IN ACCESS EXCLUSIVE MODE")
                .await
                .expect("block quote lookup");
            let store = store.clone();
            let key = GuestCredential::from_session(&session).expect("guest");
            let task = tokio::spawn(async move {
                let id = "f".repeat(32);
                if promoted {
                    owned_promoted_quote(&store, &key, &id).await.map(|_| ())
                } else {
                    owned_quote(&store, &key, &id).await.map(|_| ())
                }
            });
            wait_on(&pool, "FROM guest_quote_snapshots WHERE basket_id=").await;
            if stale {
                time.0.store(expires as u64, Ordering::SeqCst);
            }
            blocker.rollback().await.expect("release lookup");
            let result = task.await.expect("read task");
            if stale {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::Denied))
                ));
            } else {
                assert!(matches!(result, Err(QuoteStoreError::NotFound)));
            }
        }
    }
    assert_eq!(fixture::counts(&pool).await, (0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn conflicting_replay_after_wait_requires_current_guest(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    issue_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("replay-wait"),
    )
    .await
    .expect("ordinary quote");
    issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("promo-wait"),
        "COURTYARD",
    )
    .await
    .expect("promoted quote");
    let expires: i64 = sqlx::query_scalar("SELECT expires_at FROM guest_sessions")
        .fetch_one(&pool)
        .await
        .expect("guest expiry");
    let before = fixture::counts(&pool).await;
    for promoted in [false, true] {
        for stale in [false, true] {
            time.0.store(1100, Ordering::SeqCst);
            let mut blocker = pool.begin().await.expect("blocker");
            blocker
                .execute("LOCK TABLE guest_quote_snapshots IN ACCESS EXCLUSIVE MODE")
                .await
                .expect("block effect lookup");
            let store = store.clone();
            let key = GuestCredential::from_session(&session).expect("guest");
            let csrf = csrf.clone();
            let task = tokio::spawn(async move {
                let mut request = fixture::request(if promoted {
                    "promo-wait"
                } else {
                    "replay-wait"
                });
                request.expected_basket_revision = 2;
                if promoted {
                    issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
                        .await
                        .map(|_| ())
                } else {
                    issue_quote(&store, &key, &csrf, Locale::Nl, &request)
                        .await
                        .map(|_| ())
                }
            });
            wait_on(&pool, "FROM guest_quote_snapshots WHERE basket_id=").await;
            if stale {
                time.0.store(expires as u64, Ordering::SeqCst);
            }
            blocker.rollback().await.expect("release lookup");
            let result = task.await.expect("issue task");
            if stale {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::Denied))
                ));
            } else {
                assert!(matches!(result, Err(QuoteStoreError::Conflict)));
            }
            assert_eq!(fixture::counts(&pool).await, before);
            let issued: i32 = sqlx::query_scalar("SELECT issued FROM guest_quote_quota")
                .fetch_one(&pool)
                .await
                .expect("unchanged quota");
            assert_eq!(issued, 2);
        }
    }
}
