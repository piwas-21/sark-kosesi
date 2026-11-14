//! Isolated source controls for a future HTTP quote-load lab, not a load result.
use commerce_application::Locale;
use commerce_domain::quotes::QuoteError;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_quotes::{QuoteStoreError, issue_quote},
    guest_baskets::{GuestCredential, GuestError},
    quotes::QuoteFailure,
};
use sqlx::{Executor, PgPool};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod fixture;

// A fresh disposable fixture is required. Counts of these initially empty
// ledgers and the complete stock rows are independent of quote computation.
#[derive(Debug, PartialEq, Eq)]
struct Effects {
    balances: Vec<(String, i64, i64, i64)>,
    holds: i64,
    hold_lines: i64,
    hold_groups: i64,
    movements: i64,
    orders: i64,
    payments: i64,
    outbox: i64,
}

async fn effects(pool: &PgPool) -> Effects {
    let balances = sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances ORDER BY component_code")
        .fetch_all(pool).await.expect("stock rows");
    let (holds, hold_lines, hold_groups, movements, orders, payments, outbox) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM stock_holds), (SELECT count(*) FROM stock_hold_lines), (SELECT count(*) FROM basket_hold_groups), (SELECT count(*) FROM stock_movements), (SELECT count(*) FROM checkout_orders), (SELECT count(*) FROM payment_attempts), (SELECT count(*) FROM commerce_outbox)",
    ).fetch_one(pool).await.expect("empty commerce ledgers");
    Effects {
        balances,
        holds,
        hold_lines,
        hold_groups,
        movements,
        orders,
        payments,
        outbox,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn current_clock_fixture_issues_only_fresh_owned_quotes(pool: PgPool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    fixture::seed_for_load(&pool, now).await;
    let before = effects(&pool).await;
    assert_eq!(
        before.balances,
        vec![
            ("SEAT-COVER".into(), 100, 0, 0),
            ("SEAT-FOAM".into(), 100, 0, 0)
        ]
    );
    assert_eq!(
        (
            before.holds,
            before.hold_lines,
            before.hold_groups,
            before.movements,
            before.orders,
            before.payments,
            before.outbox
        ),
        (0, 0, 0, 0, 0, 0, 0)
    );
    let (store, time) = fixture::store(&pool, now);
    let (session, csrf) = fixture::guest(&store).await;
    let credential = GuestCredential::from_session(&session).expect("fixture guest");
    let request = fixture::request("load-one");
    assert!(matches!(
        issue_quote(&store, &credential, "wrong", Locale::Nl, &request).await,
        Err(QuoteStoreError::Guest(GuestError::Csrf))
    ));
    assert_eq!(fixture::counts(&pool).await, (0, 0, 0));
    let quote = issue_quote(&store, &credential, &csrf, Locale::Nl, &request)
        .await
        .expect("fresh quote");
    assert!(!quote.replayed);
    assert_eq!(
        (
            quote.snapshot.total.gross_minor,
            quote.snapshot.total.net_minor,
            quote.snapshot.total.vat_minor
        ),
        (38245, 31607, 6638)
    );
    assert_eq!(fixture::counts(&pool).await, (1, 0, 0));
    assert_eq!(effects(&pool).await, before);
    let replay = issue_quote(&store, &credential, &csrf, Locale::Nl, &request)
        .await
        .expect("replay");
    assert!(replay.replayed);
    time.0
        .store(now + 3599, std::sync::atomic::Ordering::SeqCst);
    let last = issue_quote(
        &store,
        &credential,
        &csrf,
        Locale::Nl,
        &fixture::request("last"),
    )
    .await
    .expect("last second in window");
    assert!(!last.replayed);
    time.0
        .store(now + 3600, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        issue_quote(
            &store,
            &credential,
            &csrf,
            Locale::Nl,
            &fixture::request("expired")
        )
        .await,
        Err(QuoteStoreError::Evaluation(QuoteFailure::Policy(
            QuoteError::MarketUnavailable
        )))
    ));
    assert_eq!(fixture::counts(&pool).await, (2, 0, 0));
    assert_eq!(effects(&pool).await, before);

    // Positive control: an actual balance change must trip the comparison.
    pool.execute("UPDATE stock_balances SET on_hand=on_hand-1 WHERE component_code='SEAT-COVER'")
        .await
        .expect("isolated control");
    assert_ne!(effects(&pool).await, before);
}
