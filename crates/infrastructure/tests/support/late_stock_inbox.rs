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
    assert_eq!(
        page.cases,
        vec![inbox::Summary {
            case_id: case.id.clone(),
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
