use std::sync::{Arc,atomic::{AtomicU64,Ordering}};
struct MutableClock(AtomicU64);
impl Clock for MutableClock { fn unix_seconds(&self)->Result<u64,ContextError>{Ok(self.0.load(Ordering::SeqCst))} }
async fn waiting(pool:&PgPool,pattern:&str) {
    for _ in 0..150 {
        let yes:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("observe wait");
        if yes{return;}
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }
    panic!("expected real lock wait: {pattern}");
}
#[sqlx::test(migrator="MIGRATOR")]
async fn competing_drafts_reviews_and_same_effect_retries_have_one_winner(pool:PgPool) {
    seed(&pool).await;
    let initial=policy();
    let (a,b)=tokio::join!(draft(&pool,&initial,"a"),draft(&pool,&initial,"b"));
    assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
    let (a,b)=tokio::join!(decide(&pool,"a",1,0,true),decide(&pool,"b",1,0,false));
    assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
    let mut p=policy();p.revision=2;
    let (a,b)=tokio::join!(draft(&pool,&p,"same"),draft(&pool,&p,"same"));
    assert_ne!(a.expect("first").replayed,b.expect("second").replayed);
    let (a,b)=tokio::join!(decide(&pool,"same",2,0,true),decide(&pool,"same",2,0,true));
    assert_ne!(a.expect("first").replayed,b.expect("second").replayed);
}
#[sqlx::test(migrator="MIGRATOR")]
async fn writes_and_inspection_recheck_authority_after_actual_policy_lock_waits(pool:PgPool) {
    seed(&pool).await;draft(&pool,&policy(),"first").await.expect("draft");
    for kind in [0,1,2] {
        let mut blocker=pool.begin().await.expect("blocker");
        sqlx::query("SELECT id FROM market_policy_anchors FOR UPDATE").execute(&mut *blocker).await.expect("claim");
        let c=Arc::new(MutableClock(AtomicU64::new(1100)));let clock=c.clone();let db=pool.clone();
        let task=tokio::spawn(async move {
            match kind {
                0=>{let mut p=policy();p.revision=2;record_draft(&db,&token(1),clock.as_ref(),DraftRequest{policy:&p,audit:audit("expire",1)}).await.map(|_|())},
                1=>record_review(&db,&token(2),clock.as_ref(),review("expire",1,0,true)).await.map(|_|()),
                2=>inspect_policy(&db,&token(2),clock.as_ref(),"SYNTHETIC-NL",1).await.map(|_|()),
                3=>record_draft(&db,&token(1),clock.as_ref(),DraftRequest{policy:&policy(),audit:audit("first",0)}).await.map(|_|()),
                _=>record_review(&db,&token(2),clock.as_ref(),review("existing",1,0,true)).await.map(|_|())
            }
        });
        waiting(&pool,if kind==2 {"SELECT id FROM market_policy_anchors%"} else {"UPDATE market_policy_anchors%"}).await;
        c.0.store(2001,Ordering::SeqCst);blocker.rollback().await.expect("release");
        assert!(matches!(task.await.expect("task"),Err(MarketError::Staff(StaffError::Denied))));
    }
    assert!(inspect(&pool,1).await.expect("no expired approval").review.is_none());
    assert!(matches!(inspect(&pool,2).await,Err(MarketError::NotFound)));
}
#[sqlx::test(migrator="MIGRATOR")]
async fn replay_expiry_cancellation_and_revocation_keep_effects_atomic(pool:PgPool) {
    seed(&pool).await;draft(&pool,&policy(),"first").await.expect("draft");decide(&pool,"review",1,0,true).await.expect("review");
    for is_review in [false,true] {
        let mut blocker=pool.begin().await.expect("blocker");
        let key=if is_review {"market-review:review"} else {"market-draft:first"};
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))").bind(key).execute(&mut *blocker).await.expect("effect claim");
        let c=Arc::new(MutableClock(AtomicU64::new(1100)));let clock=c.clone();let db=pool.clone();
        let task=tokio::spawn(async move { if is_review {record_review(&db,&token(2),clock.as_ref(),review("review",1,0,true)).await} else {record_draft(&db,&token(1),clock.as_ref(),DraftRequest{policy:&policy(),audit:audit("first",0)}).await} });
        waiting(&pool,"SELECT pg_advisory_xact_lock%").await;c.0.store(2001,Ordering::SeqCst);blocker.rollback().await.expect("release");
        assert!(matches!(task.await.expect("task"),Err(MarketError::Staff(StaffError::Denied))));
    }
    let mut blocker=pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM market_policy_anchors FOR UPDATE").execute(&mut *blocker).await.expect("claim");
    let db=pool.clone();let task=tokio::spawn(async move {decide(&db,"cancel",1,1,false).await});
    waiting(&pool,"UPDATE market_policy_anchors%").await;task.abort();assert!(task.await.expect_err("cancel").is_cancelled());blocker.rollback().await.expect("release");
    decide(&pool,"after-cancel",1,1,false).await.expect("no leaked authority/anchor locks");
    pool.execute("DELETE FROM staff_capabilities WHERE staff_id='reviewer'").await.expect("revoke");
    assert!(matches!(decide(&pool,"review",1,0,true).await,Err(MarketError::Staff(StaffError::Denied))));
}
