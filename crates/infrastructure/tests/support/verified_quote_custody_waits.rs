//! Customer authority must remain fresh when quote source lookups fail.
use super::*;
use commerce_domain::quotes::QuoteError;
use commerce_infrastructure::quotes::QuoteFailure;

async fn wait_on(pool: &PgPool, fragment: &str) {
    for _ in 0..150 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND cardinality(pg_blocking_pids(pid))>0 AND query LIKE $1)",
        )
        .bind(format!("%{fragment}%"))
        .fetch_one(pool)
        .await
        .expect("observe lock wait");
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(4)).await;
    }
    panic!("expected quote query did not wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failure_after_wait_requires_current_customer_even_with_current_guest(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (account_id, customer) = account(&pool, "custody@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let customer_expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM customer_sessions WHERE account_id=$1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("customer expiry");
    let guest_expiry: i64 = sqlx::query_scalar("SELECT expires_at FROM guest_sessions")
        .fetch_one(&pool)
        .await
        .expect("guest expiry");
    assert!(customer_expiry < guest_expiry);
    let before = effects(&pool).await;
    for read in [false, true] {
        for stale in [false, true] {
            time.0.store(1100, Ordering::SeqCst);
            pool.execute("UPDATE stock_balances SET on_hand=100 WHERE component_code='SEAT-COVER'")
                .await
                .expect("reset stock");
            let mut blocker = pool.begin().await.expect("blocker");
            if read {
                blocker
                    .execute("LOCK TABLE guest_quote_snapshots IN ACCESS EXCLUSIVE MODE")
                    .await
                    .expect("block quote read");
            } else {
                sqlx::query(
                    "UPDATE stock_balances SET on_hand=0 WHERE component_code='SEAT-COVER'",
                )
                .execute(&mut *blocker)
                .await
                .expect("uncommitted scarcity");
            }
            let store = store.clone();
            let key = GuestCredential::from_session(&session).expect("guest");
            let customer = CustomerSessionToken::from_bytes(*customer.expose_bytes());
            let csrf = csrf.clone();
            let task = tokio::spawn(async move {
                if read {
                    owned_verified_promoted_quote(&store, &key, &customer, &"f".repeat(32))
                        .await
                        .map(|_| ())
                } else {
                    issue_verified_promoted_quote(
                        &store,
                        &key,
                        &csrf,
                        &customer,
                        Locale::Nl,
                        &fixture::request("wait-verified"),
                        "COURTYARD",
                    )
                    .await
                    .map(|_| ())
                }
            });
            wait_on(
                &pool,
                if read {
                    "FROM guest_quote_snapshots WHERE basket_id="
                } else {
                    "SELECT component_code,on_hand,"
                },
            )
            .await;
            if stale {
                time.0.store(customer_expiry as u64, Ordering::SeqCst);
            }
            if read {
                blocker.rollback().await.expect("release read");
            } else {
                blocker.commit().await.expect("expose scarcity");
            }
            let result = task.await.expect("quote task");
            if stale {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::Denied))
                ));
            } else if read {
                assert!(matches!(result, Err(QuoteStoreError::NotFound)));
            } else {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Evaluation(QuoteFailure::Policy(
                        QuoteError::InsufficientStock
                    )))
                ));
            }
            assert_eq!(effects(&pool).await, before);
        }
    }
}
