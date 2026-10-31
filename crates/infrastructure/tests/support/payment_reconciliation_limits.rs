use super::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn paid_or_ambiguous_new_facts_during_poll_wait_stop_without_another_get(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "pending")], None),
        fetched(f.snapshot(), "tr_A", "pending"),
    ]);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("deferred poll");
    crate::payment_attempts::observations::record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "other-paid",
        &super::super::observation_cases::fetched(f.snapshot(), "tr_B", "paid", true).await,
    )
    .await
    .expect("independently fetched fact");
    time.0.store(1560, Ordering::SeqCst);
    let b = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("ambiguity stop");
    assert_eq!(b.needs_review, 1);
    assert!(matches!(
        read_job(&pool, a)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::Ambiguous)
    ));
    assert_eq!(server.count(), 3);
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn previously_paid_identity_is_not_polled_and_unknown_poll_state_is_not_absence(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let server = Server::start(vec![profile(), page(f.snapshot(), &[], None)]);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &i, &client, 1)
        .await
        .expect("resumable page checkpoint");
    pool.execute("UPDATE payment_reconciliation_jobs SET phase='poll'")
        .await
        .expect("operator recovery fixture");
    let lease = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("lease")
        .expect("found");
    let no_identity = steps::advance(&pool, time.as_ref(), &i, &client, &lease)
        .await
        .expect("unresolved");
    assert!(matches!(
        no_identity.review_reason,
        Some(ReviewReason::Unresolved)
    ));
    crate::payment_attempts::observations::record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "paid",
        &super::super::observation_cases::fetched(f.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("paid fact");
    // A fresh generation resolves the newly arrived evidence, preserving old receipts.
    time.0.store(1560, Ordering::SeqCst);
    let b = run_batch(&pool, time.as_ref(), &i, &client, 1)
        .await
        .expect("retain paid");
    assert_eq!(b.needs_review, 1);
    assert_eq!(server.count(), 2);
    assert!(matches!(
        read_job(&pool, a)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::PaidObserved)
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn scan_cap_preserves_continuation_and_escalates_without_automatic_restart(pool: PgPool) {
    use commerce_domain::payment_attempts::searches::SearchSnapshot;
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let server = Server::start(vec![profile(), page(f.snapshot(), &[], Some("tr_1"))]);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &i, &client, 1)
        .await
        .expect("authenticated first page");
    let value: Value = sqlx::query_scalar("SELECT snapshot FROM payment_search_pages")
        .fetch_one(&pool)
        .await
        .expect("original");
    let mut previous: SearchSnapshot = serde_json::from_value(value).expect("closed");
    let mut tx = pool.begin().await.expect("bounded native chain");
    for n in 2..=100 {
        let mut s = previous.clone();
        s.id = format!("{:032x}", n + 500000);
        s.effect_key = format!("fixture-page-{n}");
        s.revision = n;
        s.page_number = n;
        s.intent.previous_page_id = Some(previous.id.clone());
        s.intent.from = previous.intent.next.clone();
        s.intent.next = Some(format!("tr_{n}"));
        sqlx::query("INSERT INTO payment_search_pages(id,attempt_id,effect_key,revision,page_number,recorded_at,previous_page_id,from_cursor,next_cursor,candidates,credential_fingerprint,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)").bind(&s.id).bind(a).bind(&s.effect_key).bind(s.revision).bind(s.page_number).bind(s.recorded_at).bind(&s.intent.previous_page_id).bind(&s.intent.from).bind(&s.intent.next).bind(json!([])).bind(&s.intent.credential_fingerprint).bind(json!(s)).execute(&mut *tx).await.expect("exact page");
        let order = &f.snapshot().binding.order_id;
        sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.search_page_recorded.v1',$3,1500)").bind(format!("{:032x}",n+600000)).bind(order).bind(json!({"schema_version":1,"order_id":order,"attempt_id":a,"page_id":s.id})).execute(&mut *tx).await.expect("page event");
        previous = s;
    }
    tx.commit().await.expect("native complete chain");
    sqlx::query("UPDATE payment_reconciliation_jobs SET page_id=$1")
        .bind(&previous.id)
        .execute(&pool)
        .await
        .expect("resumed exact checkpoint");
    let mut lease = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("claim")
        .expect("lease");
    lease.job.phase = Phase::Search;
    let stopped = steps::advance(&pool, time.as_ref(), &i, &client, &lease)
        .await
        .expect("preflight scan cap");
    assert!(matches!(
        stopped.review_reason,
        Some(ReviewReason::ScanLimit)
    ));
    time.0.store(1560, Ordering::SeqCst);
    let b = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("bounded stop");
    assert_eq!(b.needs_review, 1);
    let job = read_job(&pool, a).await.expect("job").expect("found");
    assert!(matches!(job.review_reason, Some(ReviewReason::ScanLimit)));
    assert_eq!(job.page_id, Some(previous.id));
    assert_eq!(server.count(), 2);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reconstructed_poll_checkpoint_cannot_cross_the_provider_call_limit(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let i = ids();
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "pending")], None),
        fetched(f.snapshot(), "tr_A", "pending"),
    ]);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("waiting poll");
    time.0.store(1560, Ordering::SeqCst);
    let mut lease = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("claim")
        .expect("lease");
    lease.job.polls = 10;
    let stopped = steps::advance(&pool, time.as_ref(), &i, &client, &lease)
        .await
        .expect("preflight bound");
    assert!(matches!(
        stopped.review_reason,
        Some(ReviewReason::PollLimit)
    ));
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn another_paid_identity_arriving_during_poll_get_is_included_before_checkpoint(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = f.snapshot().binding.id.clone();
    let i = Arc::new(ids());
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut pending = payment(f.snapshot());
    pending["id"] = "tr_A".into();
    pending["status"] = "pending".into();
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "pending")], None),
        fetched(f.snapshot(), "tr_A", "pending"),
        Reply::GateJson(gate.clone(), pending),
    ]);
    let client = Arc::new(read_client(&server).await);
    run_batch(&pool, time.as_ref(), i.as_ref(), client.as_ref(), 25)
        .await
        .expect("initial scan");
    time.0.store(1560, Ordering::SeqCst);
    let p = pool.clone();
    let t = time.clone();
    let ids = i.clone();
    let v = client.clone();
    let task =
        tokio::spawn(async move { run_batch(&p, t.as_ref(), ids.as_ref(), v.as_ref(), 1).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 4 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("poll GET in progress");
    crate::payment_attempts::observations::record_verified(
        &pool,
        time.as_ref(),
        i.as_ref(),
        &a,
        "concurrent-paid",
        &super::super::observation_cases::fetched(f.snapshot(), "tr_B", "paid", false).await,
    )
    .await
    .expect("independent authenticated fact");
    gate.store(true, Ordering::SeqCst);
    assert_eq!(
        task.await
            .expect("task")
            .expect("post-GET resolution")
            .needs_review,
        1
    );
    let r = crate::payment_attempts::identities::latest(&pool, &a)
        .await
        .expect("resolution")
        .expect("exists");
    assert_eq!(r.summary.provider_ids, vec!["tr_A", "tr_B"]);
    assert_eq!(r.summary.paid_provider_ids, vec!["tr_B"]);
    assert!(matches!(
        read_job(&pool, &a)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::Ambiguous)
    ));
    get_only(&server);
}
