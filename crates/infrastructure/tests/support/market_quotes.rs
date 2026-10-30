#[allow(dead_code, unused_imports)]
mod catalogue_fixture {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}
fn basket() -> QuoteRequest {
    QuoteRequest {
        destination: policy().destination,
        lines: vec![BasketLine {
            sku: "SET-FILLED".into(),
            quantity: 3,
        }],
    }
}
async fn ready_catalogue(pool: &PgPool) {
    use catalogue_fixture as f;
    let _case = f::media_seed(pool).await;
    f::approve(pool).await;
    commerce_infrastructure::publication::record_publication(
        pool,
        &f::author(),
        &FixedClock(1100),
        commerce_infrastructure::publication::PublicationRequest {
            locale: commerce_application::Locale::Nl,
            evidence_ref: "synthetic",
            sku: "SET-FILLED",
            expected_revision: 0,
            effect_key: "activate",
            choice: &f::choice(),
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("publish synthetic");
    pool.execute("INSERT INTO stock_balances(component_code,on_hand,quarantine,reserved) VALUES('SEAT-COVER',100,0,0),('SEAT-FOAM',100,0,0)").await.expect("synthetic stock");
    pool.execute("INSERT INTO staff_capabilities VALUES('author','market.manage'),('reviewer','market.review'),('other','market.review')").await.expect("isolated market grants");
}
async fn q(pool: &PgPool) -> Result<commerce_infrastructure::quotes::ServerQuote, MarketError> {
    reviewed_quote(
        pool,
        &FixedClock(1100),
        commerce_application::Locale::Nl,
        &basket(),
    )
    .await
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quotes_use_only_latest_independently_reviewed_exact_destination_policy(pool: PgPool) {
    ready_catalogue(&pool).await;
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
    draft(&pool, &policy(), "first").await.expect("draft");
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
    decide(&pool, "review", 1, 0, true).await.expect("review");
    let quote = q(&pool).await.expect("reviewed quote");
    assert_eq!(
        (
            quote.policy_revision,
            quote.totals.total.gross.minor_units(),
            quote.totals.total.net.minor_units(),
            quote.totals.total.vat.minor_units()
        ),
        (1, 38245, 31607, 6638)
    );
    let mut wrong = basket();
    wrong.destination.postcode = "1234 AC".into();
    assert!(matches!(
        reviewed_quote(
            &pool,
            &FixedClock(1100),
            commerce_application::Locale::Nl,
            &wrong
        )
        .await,
        Err(MarketError::NotFound)
    ));
    wrong.destination.country = "DE".into();
    assert!(matches!(
        reviewed_quote(
            &pool,
            &FixedClock(1100),
            commerce_application::Locale::Nl,
            &wrong
        )
        .await,
        Err(MarketError::NotFound)
    ));
    wrong.lines.clear();
    assert!(matches!(
        reviewed_quote(
            &pool,
            &FixedClock(1100),
            commerce_application::Locale::Nl,
            &wrong
        )
        .await,
        Err(MarketError::Quote(_))
    ));
    let mut p = policy();
    p.revision = 2;
    p.shipping.gross = Money::eur(999).expect("synthetic");
    draft(&pool, &p, "new").await.expect("new draft");
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
    assert!(
        decide(&pool, "review", 1, 0, true)
            .await
            .expect("old receipt")
            .replayed
    );
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
    decide(&pool, "new-review", 2, 0, true)
        .await
        .expect("new exact review");
    assert_eq!(
        (
            q(&pool).await.expect("new quote").policy_revision,
            q(&pool)
                .await
                .expect("total")
                .totals
                .total
                .gross
                .minor_units()
        ),
        (2, 38034)
    );
    decide(&pool, "withdraw", 2, 1, false)
        .await
        .expect("withdraw");
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
    decide(&pool, "new-review", 2, 0, true)
        .await
        .expect("historical receipt");
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)",
    )
    .fetch_one(&pool)
    .await
    .expect("no financial or stock effects");
    assert_eq!(counts, (0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn policy_expiry_during_stock_wait_and_withdrawal_ordering_are_atomic(pool: PgPool) {
    ready_catalogue(&pool).await;
    let mut p = policy();
    p.expires_at = 1200;
    draft(&pool, &p, "first").await.expect("draft");
    decide(&pool, "review", 1, 0, true).await.expect("review");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT component_code FROM stock_balances FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("stock claim");
    let c = Arc::new(MutableClock(AtomicU64::new(1100)));
    let clock = c.clone();
    let db = pool.clone();
    let quote = tokio::spawn(async move {
        reviewed_quote(
            &db,
            clock.as_ref(),
            commerce_application::Locale::Nl,
            &basket(),
        )
        .await
    });
    waiting(&pool, "SELECT component_code,on_hand,%").await;
    let db = pool.clone();
    let withdrawal = tokio::spawn(async move { decide(&db, "withdraw", 1, 1, false).await });
    waiting(&pool, "UPDATE market_policy_anchors%").await;
    c.0.store(1200, Ordering::SeqCst);
    blocker.rollback().await.expect("release");
    assert!(matches!(
        quote.await.expect("quote"),
        Err(MarketError::Quote(
            commerce_infrastructure::quotes::QuoteFailure::Policy(QuoteError::MarketUnavailable)
        ))
    ));
    withdrawal
        .await
        .expect("withdrawal task")
        .expect("withdrawal follows quote");
    assert!(matches!(q(&pool).await, Err(MarketError::NotFound)));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn database_outage_never_returns_review_or_quote_data(pool: PgPool) {
    pool.close().await;
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(MarketError::Staff(StaffError::Database(_)))
    ));
    assert!(matches!(q(&pool).await, Err(MarketError::Quote(_))));
}
