//! Staff read controls use genuine late-stock custody, not synthetic summaries.
use super::*;
use crate::{
    late_stock::inbox::{self, InboxError, MAX_CANDIDATES},
    staff_access::{SessionToken, StaffError},
};

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_review_inbox_authority_bounded_paging_filter_and_damaged_custody(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let hold = &original_holds(&pool, &f).await[0];
    transition_hold(
        &pool,
        hold,
        HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("real release");
    f.time.0.store(2000, Ordering::SeqCst);
    let receipt = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine review");
    let Outcome::NeedsReview(case) = receipt.outcome else {
        panic!("review stop")
    };
    pool.execute("INSERT INTO staff_accounts VALUES('stock-inbox-reader',true),('stock-inbox-warehouse',true); INSERT INTO staff_capabilities VALUES('stock-inbox-reader','stock.review'),('stock-inbox-warehouse','warehouse.manage')").await.expect("isolated grants");
    for (number, actor) in [(77, "stock-inbox-reader"), (78, "stock-inbox-warehouse")] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1900)")
            .bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(&pool).await.expect("isolated MFA");
    }
    let token = SessionToken::from_bytes([77; 32]);
    let read =
        |after, reason, limit| inbox::list(&pool, &token, f.time.as_ref(), after, reason, limit);
    let before = all_effects(&pool).await;
    let page = read(None, None, 1)
        .await
        .expect("authorized complete custody");
    let case_digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(c) FROM late_stock_review_cases c WHERE id=$1",
    )
    .bind(&case.id)
    .fetch_one(&pool)
    .await
    .expect("exact review digest");
    assert_eq!(
        page.cases,
        vec![inbox::Summary {
            case_id: case.id.clone(),
            case_digest,
            status: inbox::CaseStatus::Open,
            next_generation: None,
            order_id: case.order_id.clone(),
            assessed_at: 2000,
            reason: ReviewReason::ReleaseIntentUnresolved
        }]
    );
    assert_eq!(page.next_cursor.as_deref(), Some(case.id.as_str()));
    assert!(
        read(page.next_cursor.as_deref(), None, 1)
            .await
            .expect("end page")
            .cases
            .is_empty()
    );
    let filtered = read(None, Some(ReviewReason::ComponentScarcity), 1)
        .await
        .expect("nonmatching filter");
    assert!(filtered.cases.is_empty());
    assert_eq!(filtered.next_cursor, Some(case.id.clone()));
    assert_eq!(
        read(
            None,
            Some(ReviewReason::ReleaseIntentUnresolved),
            MAX_CANDIDATES
        )
        .await
        .expect("matching filter")
        .cases
        .len(),
        1
    );
    for limit in [0, MAX_CANDIDATES + 1, u8::MAX] {
        assert!(matches!(
            read(None, None, limit).await,
            Err(InboxError::InvalidRequest)
        ));
    }
    assert!(matches!(
        read(Some("invalid"), None, 1).await,
        Err(InboxError::InvalidRequest)
    ));
    assert!(matches!(
        inbox::list(
            &pool,
            &SessionToken::from_bytes([78; 32]),
            f.time.as_ref(),
            None,
            None,
            1
        )
        .await,
        Err(InboxError::Staff(StaffError::Denied))
    ));
    assert_eq!(
        all_effects(&pool).await,
        before,
        "staff reads have no physical or journal effect"
    );
    let mut plan_tx = pool.begin().await.expect("index plan");
    plan_tx
        .execute("SET LOCAL enable_seqscan=off")
        .await
        .expect("inspect index eligibility");
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT id FROM late_stock_review_cases WHERE id COLLATE \"C\" > $1 COLLATE \"C\" ORDER BY id COLLATE \"C\" LIMIT 4")
        .bind("0".repeat(32)).fetch_all(&mut *plan_tx).await.expect("range seek plan");
    assert!(
        plan.iter()
            .any(|line| line.contains("late_stock_review_inbox_id_c") && line.contains("Index")),
        "{plan:?}"
    );
    assert!(
        plan.iter().any(|line| line.contains("Index Cond")),
        "{plan:?}"
    );
    plan_tx.rollback().await.expect("end explain");
    pool.execute("ALTER TABLE late_stock_review_cases RENAME TO unavailable_review_cases")
        .await
        .expect("make the candidate query unavailable");
    assert!(matches!(
        read(None, None, MAX_CANDIDATES).await,
        Err(InboxError::Custody(LateStockError::Database))
    ));
    pool.execute("ALTER TABLE unavailable_review_cases RENAME TO late_stock_review_cases")
        .await
        .expect("restore the candidate query");
    assert_eq!(
        read(None, None, MAX_CANDIDATES)
            .await
            .expect("healthy read after restoring the table")
            .cases[0]
            .case_id,
        case.id
    );
    f.time.0.store(2801, Ordering::SeqCst);
    assert!(matches!(
        read(None, None, 1).await,
        Err(InboxError::Staff(StaffError::Denied))
    ));
    f.time.0.store(2000, Ordering::SeqCst);
    let mut tx = pool.begin().await.expect("controlled damaged event");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='order.stock_recovery_review.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("controlled corruption");
    tx.commit().await.expect("commit damage");
    assert!(
        matches!(
            read(None, Some(ReviewReason::ComponentScarcity), 1).await,
            Err(InboxError::Custody(LateStockError::InvalidStoredData))
        ),
        "filter must not hide damaged scanned row"
    );
    assert_ne!(
        all_effects(&pool).await,
        before,
        "damage control changed state"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_review_inbox_scans_multiple_cases_before_filtering_without_skips(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = crate::payment_test_fixture::quote::store(&pool, 1100);
    let mut fixtures = Vec::new();
    for index in 0..4 {
        time.0.store(1100, Ordering::SeqCst);
        let f = accepted_with_store(&pool, store.clone(), time.clone(), true, false).await;
        observe(&pool, &f, &format!("tr_INBOX{index}"), "paid", "paid").await;
        recognize(
            &pool,
            time.as_ref(),
            f.ids.as_ref(),
            &f.frozen.snapshot().binding.id,
        )
        .await
        .expect("distinct received proof");
        fixtures.push(f);
    }
    let mut expected = Vec::new();
    for (index, f) in fixtures.iter().enumerate() {
        let hold = &original_holds(&pool, f).await[0];
        let command = if index % 2 == 0 {
            HoldCommand::Release {
                owner_id: &f.order.identity.id,
                actor_id: "fixture-release",
            }
        } else {
            HoldCommand::Consume {
                owner_id: &f.order.identity.id,
                actor_id: "fixture-consume",
            }
        };
        transition_hold(&pool, hold, command, 1500)
            .await
            .expect("genuine distinct stop");
        time.0.store(2000, Ordering::SeqCst);
        let receipt = recover(&pool, time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("record stop");
        let Outcome::NeedsReview(case) = receipt.outcome else {
            panic!("review stop")
        };
        let reason = if index % 2 == 0 {
            ReviewReason::ReleaseIntentUnresolved
        } else {
            ReviewReason::PriorConsumptionWithoutAllocation
        };
        assert_eq!(case.reason, reason);
        expected.push((case.id, case.order_id, reason));
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(expected.windows(2).all(|pair| pair[0].0 < pair[1].0));
    pool.execute("INSERT INTO staff_accounts VALUES('inbox-multi',true); INSERT INTO staff_capabilities VALUES('inbox-multi','stock.review')").await.expect("isolated grant");
    let token = SessionToken::from_bytes([79; 32]);
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'inbox-multi',1000,4600,1900)")
        .bind(token.digest().as_slice()).execute(&pool).await.expect("isolated session");
    let before = all_effects(&pool).await;
    let mut cursor = None;
    let mut seen = Vec::new();
    for chunk in expected.chunks(2) {
        let page = inbox::list(
            &pool,
            &token,
            time.as_ref(),
            cursor.as_deref(),
            Some(ReviewReason::ReleaseIntentUnresolved),
            2,
        )
        .await
        .expect("verified filtered page");
        let wanted: Vec<_> = chunk
            .iter()
            .filter(|item| item.2 == ReviewReason::ReleaseIntentUnresolved)
            .map(|item| (item.0.clone(), item.1.clone()))
            .collect();
        let actual: Vec<_> = page
            .cases
            .iter()
            .map(|item| (item.case_id.clone(), item.order_id.clone()))
            .collect();
        assert_eq!(actual, wanted);
        assert_eq!(page.next_cursor.as_deref(), Some(chunk[1].0.as_str()));
        seen.extend(actual);
        cursor = page.next_cursor;
    }
    assert_eq!(seen.len(), 2);
    let end = inbox::list(&pool, &token, time.as_ref(), cursor.as_deref(), None, 2)
        .await
        .expect("end");
    assert!(end.cases.is_empty() && end.next_cursor.is_none());
    assert_eq!(all_effects(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_review_inbox_rechecks_mfa_after_waiting_for_historical_custody(pool: PgPool) {
    let f = accepted(&pool, true, true).await;
    let hold = &original_holds(&pool, &f).await[0];
    transition_hold(
        &pool,
        hold,
        HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("release");
    f.time.0.store(2000, Ordering::SeqCst);
    recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("review");
    pool.execute("INSERT INTO staff_accounts VALUES('inbox-wait',true); INSERT INTO staff_capabilities VALUES('inbox-wait','stock.review')").await.expect("grant");
    let token = SessionToken::from_bytes([80; 32]);
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'inbox-wait',1000,4600,1900)")
        .bind(token.digest().as_slice()).execute(&pool).await.expect("session");
    let before = all_effects(&pool).await;
    let mut blocker = pool.begin().await.expect("custody gate");
    sqlx::query("SELECT id FROM basket_hold_groups WHERE id=$1 FOR UPDATE")
        .bind(&f.order.identity.stock_group_id)
        .fetch_one(&mut *blocker)
        .await
        .expect("lock actual original group");
    let worker_pool = pool.clone();
    let time = f.time.clone();
    let task = tokio::spawn(async move {
        inbox::list(&worker_pool, &token, time.as_ref(), None, None, 1).await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count:i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT %FROM basket_hold_groups%FOR SHARE'").fetch_one(&pool).await.expect("observed proof wait");
            if count > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("reader reached real custody wait");
    f.time.0.store(2801, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert!(matches!(
        task.await.expect("reader task"),
        Err(InboxError::Staff(StaffError::Denied))
    ));
    assert_eq!(all_effects(&pool).await, before);
    f.time.0.store(2000, Ordering::SeqCst);
    let healthy = inbox::list(
        &pool,
        &SessionToken::from_bytes([80; 32]),
        f.time.as_ref(),
        None,
        None,
        1,
    )
    .await
    .expect("fresh MFA control");
    assert_eq!(healthy.cases.len(), 1);
}
