use std::sync::Arc;
struct MutableClock(std::sync::atomic::AtomicU64);
impl Clock for MutableClock {
    fn unix_seconds(&self)->Result<u64,ContextError> {Ok(self.0.load(Ordering::SeqCst))}
}
async fn wait_for_stock_lock(pool:&PgPool) {
    for _ in 0..100 {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT component_code,on_hand,%')").fetch_one(pool).await.expect("lock observer");
        if waiting {return;}
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("quote never waited on physical stock");
}
#[sqlx::test(migrator="MIGRATOR")]
async fn reservation_committing_during_quote_wait_is_seen_and_cancellation_releases_claims(pool:PgPool) {
    let _case=media_seed(&pool).await;activate(&pool).await;
    let mut blocker=pool.begin().await.expect("blocker");
    sqlx::query("UPDATE stock_balances SET reserved=100 WHERE component_code='SEAT-COVER'").execute(&mut *blocker).await.expect("concurrent reservation");
    let db=pool.clone();let task=tokio::spawn(async move {quote(&db,&basket(1),&[policy()]).await});
    wait_for_stock_lock(&pool).await;blocker.commit().await.expect("commit reservation");
    rejected(task.await.expect("task"),QuoteError::InsufficientStock);
    stock(&pool,100,100,0,0).await;
    let mut blocker=pool.begin().await.expect("blocker");
    sqlx::query("SELECT component_code FROM stock_balances FOR UPDATE").execute(&mut *blocker).await.expect("claim");
    let db=pool.clone();let task=tokio::spawn(async move {quote(&db,&basket(1),&[policy()]).await});
    wait_for_stock_lock(&pool).await;task.abort();assert!(task.await.expect_err("cancel").is_cancelled());blocker.rollback().await.expect("release");
    assert!(quote(&pool,&basket(1),&[policy()]).await.is_ok());
    let withdraw=PublicationChoice::Withdraw {};let mut r=request("after-cancel",&withdraw);r.expected_revision=1;
    record_publication(&pool,&author(),&FixedClock(1100),r).await.expect("variant and asset claims released");
}
#[sqlx::test(migrator="MIGRATOR")]
async fn licence_or_market_expiry_during_real_stock_wait_is_checked_at_completion(pool:PgPool) {
    let _case=media_seed(&pool).await;activate(&pool).await;
    for market_expiry in [false,true] {
        let mut blocker=pool.begin().await.expect("blocker");
        sqlx::query("SELECT component_code FROM stock_balances FOR UPDATE").execute(&mut *blocker).await.expect("claim");
        let clock=Arc::new(MutableClock(std::sync::atomic::AtomicU64::new(1100)));
        let db=pool.clone();let c=clock.clone();let mut p=policy();if market_expiry {p.expires_at=1200;}
        let task=tokio::spawn(async move {server_quote(&db,c.as_ref(),Locale::Nl,&basket(1),&[p]).await});
        wait_for_stock_lock(&pool).await;clock.0.store(if market_expiry {1200}else{2000},Ordering::SeqCst);
        blocker.rollback().await.expect("release");
        rejected(task.await.expect("task"),if market_expiry {QuoteError::MarketUnavailable}else{QuoteError::ProductUnavailable});
    }
}
