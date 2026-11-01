//! GET-only bounded processing and persistent profile-scoped operational outcomes.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn separate_provider_identities_keep_ambiguity_visible_without_selecting_a_payment(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_INBOX_FIRST").await;
    let second = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_INBOX_SECOND").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_INBOX_FIRST", "open")),
        Reply::Json(200, response(f.snapshot(), "tr_INBOX_SECOND", "open")),
    ]);
    let c = client(&server).await;
    let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("both distinct identities retained");
    assert_eq!(batch.processed, 2);
    assert_eq!(batch.needs_review, 1);
    assert!(
        read_callback_job(&pool, &second)
            .await
            .expect("read")
            .expect("job")
            .review_reason
            == Some(ReviewReason::Ambiguous)
    );
    let resolution = identities::latest(&pool, &f.snapshot().binding.id)
        .await
        .expect("summary")
        .expect("exists");
    assert_eq!(
        resolution.summary.provider_ids,
        vec!["tr_INBOX_FIRST", "tr_INBOX_SECOND"]
    );
    assert_eq!(counts(&pool).await, (2, 2, 2, 1, 2));
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn private_inbox_fetches_once_processes_original_and_keeps_paid_review_visible_on_later_passes(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_INBOX_PAID").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_INBOX_PAID", "paid")),
    ]);
    let c = client(&server).await;
    let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("bounded private supervisor");
    assert_eq!(batch.processed, 1);
    assert_eq!(batch.needs_review, 1);
    let job = read_callback_job(&pool, &id)
        .await
        .expect("inspection")
        .expect("job");
    assert_eq!(job.generation, 1);
    assert!(job.lease_until.is_none());
    assert!(job.state == JobState::NeedsReview);
    assert!(job.review_reason == Some(ReviewReason::PaidObserved));
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(intake_counts(&pool).await, (1, 1, 1));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_receipt_processing")
            .fetch_one(&pool)
            .await
            .expect("independent receipt disposition"),
        1
    );
    let resolution = identities::latest(&pool, &f.snapshot().binding.id)
        .await
        .expect("separate identity summary")
        .expect("summary");
    assert_eq!(resolution.summary.paid_provider_ids, vec!["tr_INBOX_PAID"]);
    let later = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("unchanged later pass");
    assert_eq!(later.processed, 0);
    assert_eq!(later.needs_review, 1);
    assert_eq!(server.count(), 2);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await.4,
        2,
        "no stock/payment transition"
    );
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unknown_and_financially_mismatching_originals_remain_visible_without_fictional_order_or_observation(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_UNKNOWN_INBOX").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_UNKNOWN_INBOX", "paid")),
    ]);
    let c = client(&server).await;
    let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("unknown authentic callback retained");
    assert_eq!(batch.needs_review, 1);
    let job = read_callback_job(&pool, &id)
        .await
        .expect("read")
        .expect("job");
    assert!(job.review_reason == Some(ReviewReason::Unmatched));
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM commerce_outbox")
            .fetch_one(&pool)
            .await
            .expect("no invented order event"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT outcome FROM callback_receipt_processing")
            .fetch_one(&pool)
            .await
            .expect("visible unmatched disposition"),
        "unmatched"
    );
    assert_eq!(
        run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
            .await
            .expect("later review report")
            .needs_review,
        1
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authentic_callback_hint_with_wrong_original_amount_retains_unmatched_lookup_custody(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_BAD_AMOUNT_INBOX").await;
    let mut p = response(f.snapshot(), "tr_BAD_AMOUNT_INBOX", "paid");
    p["amount"]["value"] = "382.44".into();
    let server = Server::start(vec![profile(), Reply::Json(200, p)]);
    let c = client(&server).await;
    let result = run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("retain authentic mismatching fact");
    assert_eq!(result.needs_review, 1);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));
    assert!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .review_reason
            == Some(ReviewReason::Unmatched)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn open_original_settles_only_after_receipt_observation_and_resolution_custody(pool: PgPool) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_OPEN_INBOX").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_OPEN_INBOX", "open")),
    ]);
    let c = client(&server).await;
    let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("nonterminal evidence retained");
    assert_eq!(batch.processed, 1);
    assert_eq!(batch.needs_review, 0);
    assert_eq!(batch.retry_pending, 0);
    let job = read_callback_job(&pool, &id)
        .await
        .expect("read")
        .expect("job");
    assert!(job.state == JobState::Done);
    assert!(job.review_reason.is_none());
    assert!(job.settled);
    let (outcome, observation, resolution): (String, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT outcome,observation_id,resolution_id FROM callback_receipt_processing",
        )
        .fetch_one(&pool)
        .await
        .expect("complete linked disposition");
    assert_eq!(outcome, "observed");
    assert!(observation.is_some() && resolution.is_some());
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(server.count(), 2);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn five_lifetime_get_failures_back_off_exhaust_and_do_not_restart_after_review(pool: PgPool) {
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_RETRY_INBOX").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
    ]);
    let c = client(&server).await;
    for (generation, at, next) in [
        (1, 1500, 1560),
        (2, 1560, 1680),
        (3, 1680, 1920),
        (4, 1920, 2400),
        (5, 2400, 2400),
    ] {
        clock.0.store(at, Ordering::SeqCst);
        let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
            .await
            .expect("bounded retry pass");
        assert_eq!(batch.processed, 1);
        let job = read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job");
        assert_eq!(job.generation, generation);
        assert_eq!(job.due_at, next);
        if generation < 5 {
            assert!(job.state == JobState::Pending);
            assert_eq!(batch.retry_pending, 1);
            assert_eq!(
                run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
                    .await
                    .expect("before due")
                    .processed,
                0
            );
        } else {
            assert!(job.review_reason == Some(ReviewReason::RetryExhausted));
            assert_eq!(batch.needs_review, 1);
        }
    }
    clock.0.store(10000, Ordering::SeqCst);
    let stopped = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("persistent review stop");
    assert_eq!(stopped.processed, 0);
    assert_eq!(stopped.needs_review, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_inbox_generations")
            .fetch_one(&pool)
            .await
            .expect("all original operations retained"),
        5
    );
    assert_eq!(counts(&pool).await, (5, 0, 0, 0, 0));
    assert_eq!(server.count(), 6);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_scope_stops_without_retry_and_another_profile_remains_unclaimed(pool: PgPool) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let own = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_WRONG_SCOPE").await;
    let other = admit(&pool, clock.as_ref(), &i, "pfl_OTHER", "tr_OTHER_PROFILE").await;
    let mut p = response(f.snapshot(), "tr_WRONG_SCOPE", "paid");
    p["profileId"] = "pfl_OTHER".into();
    let server = Server::start(vec![profile(), Reply::Json(200, p)]);
    let c = client(&server).await;
    let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("bad authenticated projection stops");
    assert_eq!(batch.processed, 1);
    assert_eq!(batch.needs_review, 1);
    assert!(
        read_callback_job(&pool, &own)
            .await
            .expect("own job")
            .expect("exists")
            .review_reason
            == Some(ReviewReason::CustodyInvalid)
    );
    assert_eq!(
        read_callback_job(&pool, &other)
            .await
            .expect("foreign job")
            .expect("exists")
            .generation,
        0
    );
    assert_eq!(counts(&pool).await, (1, 0, 0, 0, 0));
    assert_eq!(server.count(), 2);
    for limit in [0, 26] {
        assert_eq!(
            run_callback_batch(&pool, clock.as_ref(), &i, &c, limit)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        run_callback_batch(&pool, &BadPorts, &i, &c, 1).await.err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        read_callback_job(&pool, "invalid").await.err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert!(
        read_callback_job(&pool, &"f".repeat(32))
            .await
            .expect("missing job")
            .is_none()
    );
}
