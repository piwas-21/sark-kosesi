//! Isolated PostgreSQL + authenticated loopback list pages; never provider acceptance.
use super::observation_cases::{BadPorts, ids, prepared};
use super::*;
use crate::payment_attempts::searches::{read_page, read_recorded, record_page};
use commerce_domain::payment_attempts::searches::SearchSnapshot;
use std::sync::Arc;
async fn listed(
    attempt: &AttemptSnapshot,
    from: Option<&str>,
    next: Option<&str>,
    candidates: &[&str],
    rotated: bool,
) -> VerifiedSearchPage {
    let payments: Vec<_> = candidates
        .iter()
        .map(|id| {
            let mut p = payment(attempt);
            p["id"] = (*id).into();
            p
        })
        .collect();
    let next = next.map_or(Value::Null, |id| {
        let mut url = url::Url::parse("https://api.mollie.com/v2/payments").expect("fixed");
        url.query_pairs_mut()
            .append_pair("from", id)
            .append_pair("limit", "10");
        json!({"href":url.as_str()})
    });
    let server = Server::start(vec![
        profile(),
        Reply::Json(
            200,
            json!({"count":payments.len(),"_embedded":{"payments":payments},"_links":{"next":next}}),
        ),
    ]);
    let client = if rotated {
        let key = TestCredential::capture(
            &format!("test_{}", "rotated".repeat(5)),
            "pfl_SYNTHETIC_NotARealProfile",
        )
        .expect("synthetic");
        let c = config(&key);
        TestClient::local(key, c, server.base.clone(), Duration::from_secs(2)).expect("loopback")
    } else {
        local(&server)
    };
    let cursor = from.map(|s| SearchCursor::capture(s).expect("bounded"));
    let page = client
        .verify_profile()
        .await
        .expect("profile")
        .search_page(attempt, cursor.as_ref())
        .await
        .expect("authenticated page");
    assert_eq!(server.count(), 2, "no automatic page/fetch/create");
    server.inspect(|rs| assert!(rs.iter().all(|r| r.method == "GET")));
    page
}
async fn saved(
    pool: &PgPool,
    time: &dyn commerce_application::request_context::Clock,
    effect: &str,
    previous: Option<&str>,
    page: &VerifiedSearchPage,
) -> SearchSnapshot {
    record_page(pool, time, &ids(), effect, previous, page)
        .await
        .expect("new page")
        .snapshot
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn durable_pages_resume_after_restart_and_keep_every_candidate_without_financial_effect(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let page = listed(
        f.snapshot(),
        None,
        Some("tr_OFFSET"),
        &["tr_ONE", "tr_TWO"],
        false,
    )
    .await;
    let first = saved(&pool, time.as_ref(), "list-a", None, &page).await;
    assert_eq!(
        (first.revision, first.page_number, first.recorded_at),
        (1, 1, 1500)
    );
    assert_eq!(first.intent.candidates, vec!["tr_ONE", "tr_TWO"]);
    assert_eq!(
        first.intent.credential_fingerprint,
        "78a0e854453c8e82b9c97187636308791541cdbbcf28965b47a6ee3e33af97ef"
    );
    let restored = read_recorded(&pool, a, "list-a")
        .await
        .expect("restart lookup")
        .expect("durable");
    assert!(restored == first);
    let replay = record_page(&pool, &BadPorts, &BadPorts, "list-a", None, &page)
        .await
        .expect("receipt independent of current ports");
    assert!(replay.replayed && replay.snapshot == first);
    time.0.store(5000, Ordering::SeqCst);
    crate::holds::expire_due_holds(&pool, 5000, 25)
        .await
        .expect("actual holds expired");
    let next = listed(
        f.snapshot(),
        restored.intent.next.as_deref(),
        None,
        &[],
        true,
    )
    .await;
    let second = record_page(
        &pool,
        time.as_ref(),
        &crate::payment_test_fixture::Ids(AtomicU64::new(30000)),
        "list-b",
        Some(&first.id),
        &next,
    )
    .await
    .expect("late rotated page")
    .snapshot;
    assert_eq!(
        (second.revision, second.page_number, second.recorded_at),
        (2, 2, 5000)
    );
    assert!(second.intent.candidates.is_empty() && second.intent.next.is_none());
    assert_ne!(
        second.intent.credential_fingerprint,
        first.intent.credential_fingerprint
    );
    assert!(
        read_page(&pool, a, &second.id)
            .await
            .expect("validated chain")
            == second
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_observations")
            .fetch_one(&pool)
            .await
            .expect("no financial facts"),
        0
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 5, 1, 1, 4)
    );
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &BadPorts, a, &config(&credential()))
            .await
            .expect("still reconcile"),
        DispatchOutcome::Reconcile { .. }
    ));
    let changed = listed(f.snapshot(), None, None, &[], false).await;
    assert_eq!(
        record_page(&pool, time.as_ref(), &ids(), "list-a", None, &changed)
            .await
            .err(),
        Some(PaymentStoreError::Conflict)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_same_operation_has_one_page_and_event_distinct_operations_remain_visible(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let page = listed(f.snapshot(), None, None, &[], false).await;
    let i = ids();
    let (a, b) = tokio::join!(
        record_page(&pool, time.as_ref(), &i, "same", None, &page),
        record_page(&pool, time.as_ref(), &i, "same", None, &page)
    );
    let a = a.expect("winner or retry");
    let b = b.expect("winner or retry");
    assert_ne!(a.replayed, b.replayed);
    assert!(a.snapshot == b.snapshot);
    let (a, b) = tokio::join!(
        record_page(&pool, time.as_ref(), &i, "scan-two", None, &page),
        record_page(&pool, time.as_ref(), &i, "scan-three", None, &page)
    );
    let mut revisions = vec![
        a.expect("scan").snapshot.revision,
        b.expect("scan").snapshot.revision,
    ];
    revisions.sort();
    assert_eq!(revisions, vec![2, 3]);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 6, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn continuation_rejects_wrong_parent_wrong_cursor_forks_and_cycles(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let i = ids();
    let first = record_page(
        &pool,
        time.as_ref(),
        &i,
        "first",
        None,
        &listed(f.snapshot(), None, Some("tr_A"), &[], false).await,
    )
    .await
    .expect("first")
    .snapshot;
    let wrong = listed(f.snapshot(), Some("tr_WRONG"), None, &[], false).await;
    assert_eq!(
        record_page(&pool, time.as_ref(), &i, "wrong", Some(&first.id), &wrong)
            .await
            .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert_eq!(
        record_page(
            &pool,
            time.as_ref(),
            &i,
            "missing",
            Some(&"f".repeat(32)),
            &wrong
        )
        .await
        .err(),
        Some(PaymentStoreError::NotFound)
    );
    assert_eq!(
        record_page(&pool, time.as_ref(), &i, "parentless", None, &wrong)
            .await
            .err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    let second = record_page(
        &pool,
        time.as_ref(),
        &i,
        "second",
        Some(&first.id),
        &listed(f.snapshot(), Some("tr_A"), Some("tr_B"), &[], false).await,
    )
    .await
    .expect("second")
    .snapshot;
    assert_eq!(
        record_page(
            &pool,
            time.as_ref(),
            &i,
            "fork",
            Some(&first.id),
            &listed(f.snapshot(), Some("tr_A"), None, &[], false).await
        )
        .await
        .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert_eq!(
        record_page(
            &pool,
            time.as_ref(),
            &i,
            "cycle",
            Some(&second.id),
            &listed(f.snapshot(), Some("tr_B"), Some("tr_A"), &[], false).await
        )
        .await
        .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    let terminal = record_page(
        &pool,
        time.as_ref(),
        &i,
        "terminal",
        Some(&second.id),
        &listed(f.snapshot(), Some("tr_B"), None, &[], false).await,
    )
    .await
    .expect("terminal")
    .snapshot;
    assert_eq!(
        record_page(
            &pool,
            time.as_ref(),
            &i,
            "after-end",
            Some(&terminal.id),
            &listed(f.snapshot(), Some("tr_B"), None, &[], false).await
        )
        .await
        .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_search_pages")
            .fetch_one(&pool)
            .await
            .expect("pages"),
        3
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn authenticated_page_must_match_entire_original_attempt_and_require_durable_dispatch(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, false).await;
    let page = listed(f.snapshot(), None, None, &[], false).await;
    assert_eq!(
        record_page(&pool, time.as_ref(), &ids(), "unstarted", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::Unavailable)
    );
    start_dispatch(
        &pool,
        time.as_ref(),
        &crate::payment_test_fixture::Ids(AtomicU64::new(10000)),
        &f.snapshot().binding.id,
        &config(&credential()),
    )
    .await
    .expect("start");
    let mut foreign = f.snapshot().clone();
    foreign.binding.amount_minor = 38244;
    let page = listed(&foreign, None, None, &[], false).await;
    assert_eq!(
        record_page(&pool, time.as_ref(), &ids(), "counterfeit", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert!(
        read_recorded(&pool, &f.snapshot().binding.id, "counterfeit")
            .await
            .expect("no effects")
            .is_none()
    );
    let page = listed(f.snapshot(), None, None, &[], false).await;
    for effect in ["", "invalid-effect!"] {
        assert_eq!(
            record_page(&pool, time.as_ref(), &ids(), effect, None, &page)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        record_page(&pool, &BadPorts, &ids(), "clock-failed", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        record_page(
            &pool,
            time.as_ref(),
            &BadPorts,
            "entropy-failed",
            None,
            &page
        )
        .await
        .err(),
        Some(PaymentStoreError::Boundary(
            crate::checkout_orders::OrderStoreError::EntropyUnavailable
        ))
    );
    time.0.store(1499, Ordering::SeqCst);
    assert_eq!(
        record_page(&pool, time.as_ref(), &ids(), "rewind", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        record_page(&pool, time.as_ref(), &ids(), "healthy", None, &page)
            .await
            .is_ok()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_clock_check_and_cancellation_roll_back_page_and_event_after_actual_write_wait(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let page = Arc::new(listed(f.snapshot(), None, None, &[], false).await);
    for cancel in [false, true] {
        time.0.store(1500, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("blocker");
        blocker
            .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
            .await
            .expect("event write blocked");
        let p = pool.clone();
        let t = time.clone();
        let v = page.clone();
        let task = tokio::spawn(async move {
            record_page(&p, t.as_ref(), &ids(), "waited", None, v.as_ref()).await
        });
        crate::payment_test_fixture::waiting(&pool).await;
        if cancel {
            task.abort();
            assert!(task.await.err().expect("cancelled").is_cancelled());
        } else {
            time.0.store(1499, Ordering::SeqCst);
            blocker.commit().await.expect("release");
            assert_eq!(
                task.await.expect("task").err(),
                Some(PaymentStoreError::ClockUnavailable)
            );
            assert!(
                read_recorded(&pool, &f.snapshot().binding.id, "waited")
                    .await
                    .expect("rollback")
                    .is_none()
            );
            continue;
        }
        blocker.rollback().await.expect("release cancellation");
        assert!(
            read_recorded(&pool, &f.snapshot().binding.id, "waited")
                .await
                .expect("rollback")
                .is_none()
        );
    }
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        record_page(&pool, time.as_ref(), &ids(), "waited", None, &page)
            .await
            .is_ok()
    );
}

async fn native_insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    s: &SearchSnapshot,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO payment_search_pages(id,attempt_id,effect_key,revision,page_number,recorded_at,previous_page_id,from_cursor,next_cursor,candidates,credential_fingerprint,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(&s.id).bind(&s.attempt_id).bind(&s.effect_key).bind(s.revision).bind(s.page_number).bind(s.recorded_at)
        .bind(&s.intent.previous_page_id).bind(&s.intent.from).bind(&s.intent.next).bind(json!(s.intent.candidates)).bind(&s.intent.credential_fingerprint).bind(json!(s)).execute(&mut **tx).await?;
    Ok(())
}
async fn event_insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    s: &SearchSnapshot,
    order: &str,
    payload: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.search_page_recorded.v1',$3,$4)")
        .bind("e".repeat(32)).bind(order).bind(payload).bind(s.recorded_at).execute(&mut **tx).await?;
    Ok(())
}
fn event(s: &SearchSnapshot, order: &str) -> Value {
    json!({"schema_version":1,"order_id":order,"attempt_id":s.attempt_id,"page_id":s.id})
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_schema_requires_minimal_contiguous_time_bound_pages_and_exact_deferred_event(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let first = saved(
        &pool,
        time.as_ref(),
        "first",
        None,
        &listed(f.snapshot(), None, Some("tr_A"), &["tr_ONE"], false).await,
    )
    .await;
    let mut second = first.clone();
    second.id = "8".repeat(32);
    second.effect_key = "second".into();
    second.revision = 2;
    second.page_number = 2;
    second.recorded_at = 1600;
    second.intent.previous_page_id = Some(first.id.clone());
    second.intent.from = Some("tr_A".into());
    second.intent.next = Some("tr_B".into());
    let order = &f.snapshot().binding.order_id;
    let mut tx = pool.begin().await.expect("missing event");
    native_insert(&mut tx, &second).await.expect("pending");
    assert!(tx.commit().await.is_err());
    for field in ["schema_version", "order_id", "attempt_id", "page_id"] {
        let mut payload = event(&second, order);
        payload[field] = json!("invalid");
        let mut tx = pool.begin().await.expect("mismatched event");
        native_insert(&mut tx, &second).await.expect("pending");
        assert!(
            event_insert(&mut tx, &second, order, payload)
                .await
                .is_err()
        );
        tx.rollback().await.expect("rollback");
    }
    let mut tx = pool.begin().await.expect("healthy native");
    native_insert(&mut tx, &second).await.expect("pending");
    event_insert(&mut tx, &second, order, event(&second, order))
        .await
        .expect("bound event");
    tx.commit().await.expect("paired positive");
    assert!(
        read_page(&pool, &f.snapshot().binding.id, &second.id)
            .await
            .expect("healthy")
            == second
    );
    let changes: [fn(&mut SearchSnapshot); 10] = [
        |s| s.revision = 4,
        |s| s.recorded_at = 1599,
        |s| s.intent.from = Some("tr_C".into()),
        |s| s.intent.next = Some("tr_A".into()),
        |s| s.intent.candidates = vec!["bad".into()],
        |s| s.intent.candidates = vec!["tr_DUP".into(); 2],
        |s| s.intent.candidates = (0..11).map(|n| format!("tr_{n}")).collect(),
        |s| s.page_number = 4,
        |s| s.intent.previous_page_id = Some("7".repeat(32)),
        |s| s.attempt_id = "7".repeat(32),
    ];
    let mut third = second.clone();
    third.id = "7".repeat(32);
    third.effect_key = "third".into();
    third.revision = 3;
    third.page_number = 3;
    third.recorded_at = 1700;
    third.intent.previous_page_id = Some(second.id.clone());
    third.intent.from = Some("tr_B".into());
    third.intent.next = None;
    for change in changes {
        let mut bad = third.clone();
        change(&mut bad);
        let mut tx = pool.begin().await.expect("negative");
        assert!(native_insert(&mut tx, &bad).await.is_err());
        tx.rollback().await.expect("rollback");
    }
    for query in [
        "UPDATE payment_search_pages SET recorded_at=recorded_at",
        "DELETE FROM payment_search_pages",
    ] {
        assert!(pool.execute(query).await.is_err());
    }
    let mut bad = third.clone();
    bad.intent.previous_page_id = Some(first.id);
    bad.intent.from = Some("tr_A".into());
    bad.page_number = 2;
    let mut tx = pool.begin().await.expect("fork");
    assert!(native_insert(&mut tx, &bad).await.is_err());
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("counterfeit snapshot");
    let mut value = json!(third);
    value["intent"]["candidates"] = json!(["tr_COUNTERFEIT"]);
    assert!(sqlx::query("INSERT INTO payment_search_pages(id,attempt_id,effect_key,revision,page_number,recorded_at,previous_page_id,from_cursor,next_cursor,candidates,credential_fingerprint,snapshot) VALUES($1,$2,'third',3,3,1700,$3,'tr_B',NULL,'[]',$4,$5)")
        .bind(&third.id).bind(&third.attempt_id).bind(&second.id).bind(&third.intent.credential_fingerprint).bind(value).execute(&mut *tx).await.is_err());
    tx.rollback().await.expect("rollback");
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 5, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_snapshots_headers_ancestors_or_missing_events_fail_receipt_reads_and_retries(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let i = ids();
    let first_page = listed(f.snapshot(), None, Some("tr_A"), &["tr_ONE"], false).await;
    let first = record_page(&pool, time.as_ref(), &i, "first", None, &first_page)
        .await
        .expect("first")
        .snapshot;
    let second_page = listed(f.snapshot(), Some("tr_A"), None, &[], false).await;
    let second = record_page(
        &pool,
        time.as_ref(),
        &i,
        "second",
        Some(&first.id),
        &second_page,
    )
    .await
    .expect("second")
    .snapshot;
    for query in [
        "UPDATE payment_search_pages SET snapshot=jsonb_set(snapshot,'{intent,candidates}','[\"tr_FORGED\"]') WHERE effect_key='first'",
        "UPDATE payment_search_pages SET snapshot=snapshot||jsonb_build_object('extra',true) WHERE effect_key='first'",
        "UPDATE payment_search_pages SET credential_fingerprint=repeat('e',64) WHERE effect_key='first'",
        "UPDATE payment_search_pages SET snapshot=jsonb_set(snapshot,'{intent}','[]') WHERE effect_key='first'",
        "DELETE FROM commerce_outbox WHERE kind='payment.search_page_recorded.v1' AND payload->>'page_id'=(SELECT id FROM payment_search_pages WHERE effect_key='first')",
    ] {
        let mut tx = pool.begin().await.expect("isolated import");
        tx.execute("ALTER TABLE payment_search_pages DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER").await.expect("test-only damage");
        tx.execute(query).await.expect("controlled damage");
        // Exercise exact stored readers directly while only this test transaction sees damage.
        let (frozen, at) =
            crate::payment_attempts::evidence::context(&mut tx, &f.snapshot().binding.id, false)
                .await
                .expect("original");
        assert_eq!(
            crate::payment_attempts::searches::rows::load(
                &mut tx,
                &frozen,
                at,
                Some("first"),
                None
            )
            .await
            .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            crate::payment_attempts::searches::rows::load(
                &mut tx,
                &frozen,
                at,
                None,
                Some(&second.id)
            )
            .await
            .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        tx.rollback().await.expect("exact fixture restore");
        assert!(
            read_page(&pool, &f.snapshot().binding.id, &second.id)
                .await
                .is_ok()
        );
    }
    // Committed damage proves public store lookup/retry paths cannot fall back.
    pool.execute("ALTER TABLE payment_search_pages DISABLE TRIGGER USER; UPDATE payment_search_pages SET snapshot=jsonb_set(snapshot,'{revision}','3') WHERE effect_key='first'; ALTER TABLE payment_search_pages ENABLE TRIGGER USER").await.expect("owned damaged fixture");
    assert_eq!(
        read_recorded(&pool, &f.snapshot().binding.id, "first")
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    for effect in ["first", "new-after-damage"] {
        assert_eq!(
            record_page(&pool, time.as_ref(), &i, effect, None, &first_page)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
    pool.execute("ALTER TABLE payment_search_pages DISABLE TRIGGER USER")
        .await
        .expect("restore");
    sqlx::query("UPDATE payment_search_pages SET snapshot=$1 WHERE id=$2")
        .bind(json!(first))
        .bind(&first.id)
        .execute(&pool)
        .await
        .expect("exact restore");
    pool.execute("ALTER TABLE payment_search_pages ENABLE TRIGGER USER")
        .await
        .expect("guards restored");
    assert!(
        record_page(&pool, &BadPorts, &BadPorts, "first", None, &first_page)
            .await
            .expect("healthy retry")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_hundred_page_scan_remains_uncertain_and_rejects_automatic_continuation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let first = saved(
        &pool,
        time.as_ref(),
        "first",
        None,
        &listed(f.snapshot(), None, Some("tr_1"), &[], false).await,
    )
    .await;
    let mut previous = first;
    let mut tx = pool.begin().await.expect("bounded native chain");
    for n in 2..=100 {
        let mut s = previous.clone();
        s.id = format!("{n:032x}");
        s.effect_key = format!("page-{n}");
        s.revision = n;
        s.page_number = n;
        s.intent.previous_page_id = Some(previous.id.clone());
        s.intent.from = previous.intent.next.clone();
        s.intent.next = Some(format!("tr_{n}"));
        native_insert(&mut tx, &s).await.expect("valid chain");
        sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.search_page_recorded.v1',$3,$4)")
            .bind(format!("{:032x}",n+50000)).bind(&f.snapshot().binding.order_id).bind(event(&s,&f.snapshot().binding.order_id)).bind(s.recorded_at).execute(&mut *tx).await.expect("event");
        previous = s;
    }
    tx.commit().await.expect("all exact effects");
    assert_eq!(
        read_page(&pool, &f.snapshot().binding.id, &previous.id)
            .await
            .expect("bounded chain")
            .page_number,
        100
    );
    assert_eq!(
        record_page(
            &pool,
            time.as_ref(),
            &crate::payment_test_fixture::Ids(AtomicU64::new(40000)),
            "over-cap",
            Some(&previous.id),
            &listed(f.snapshot(), Some("tr_100"), Some("tr_101"), &[], false).await
        )
        .await
        .err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert!(
        read_recorded(&pool, &f.snapshot().binding.id, "over-cap")
            .await
            .expect("no effect")
            .is_none()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn repeatable_read_connection_defaults_cannot_hide_pages_committed_during_claim_wait(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let page = Arc::new(listed(f.snapshot(), None, None, &[], false).await);
    let ids = Arc::new(ids());
    let rr = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .min_connections(2)
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_isolation='repeatable read'")
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("owned configured pool");
    let mut blocker = pool.begin().await.expect("hold claim");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.snapshot().binding.id)
        .fetch_one(&mut *blocker)
        .await
        .expect("claim");
    let mut tasks = vec![];
    for effect in ["rr-a", "rr-b"] {
        let p = rr.clone();
        let t = time.clone();
        let v = page.clone();
        let i = ids.clone();
        tasks.push(tokio::spawn(async move {
            record_page(&p, t.as_ref(), i.as_ref(), effect, None, v.as_ref()).await
        }));
    }
    tokio::time::timeout(Duration::from_millis(800),async{loop{let count:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts WHERE id=%FOR UPDATE'").fetch_one(&pool).await.expect("claims");if count==2{break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("both real claim waits");
    blocker.commit().await.expect("release");
    let mut revisions = vec![];
    for task in tasks {
        revisions.push(
            task.await
                .expect("task")
                .expect("fresh committed view")
                .snapshot
                .revision,
        );
    }
    revisions.sort();
    assert_eq!(revisions, vec![1, 2]);
    rr.close().await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_invalid_oversized_or_unavailable_reads_never_fall_back(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let page = listed(f.snapshot(), None, None, &[], false).await;
    let first = saved(&pool, time.as_ref(), "first", None, &page).await;
    for (a, id) in [("bad", "bad"), (&f.snapshot().binding.id, "bad")] {
        assert_eq!(
            read_page(&pool, a, id).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        read_page(&pool, &f.snapshot().binding.id, &"f".repeat(32))
            .await
            .err(),
        Some(PaymentStoreError::NotFound)
    );
    assert_eq!(
        read_recorded(&pool, &"f".repeat(32), "first").await.err(),
        Some(PaymentStoreError::NotFound)
    );
    time.0.store(1600, Ordering::SeqCst);
    record_page(
        &pool,
        time.as_ref(),
        &crate::payment_test_fixture::Ids(AtomicU64::new(30000)),
        "new-at-1600",
        None,
        &page,
    )
    .await
    .expect("new scan");
    time.0.store(1599, Ordering::SeqCst);
    assert_eq!(
        record_page(&pool, time.as_ref(), &ids(), "old-clock", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    time.0.store(9_007_199_254_740_992, Ordering::SeqCst);
    assert_eq!(
        record_page(&pool, time.as_ref(), &ids(), "oversized-clock", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    for (drop_sql, update_sql) in [
        (
            "ALTER TABLE payment_search_pages DROP CONSTRAINT payment_search_pages_snapshot_check",
            "UPDATE payment_search_pages SET snapshot=snapshot||jsonb_build_object('padding',repeat('x',8193))",
        ),
        (
            "ALTER TABLE payment_search_pages DROP CONSTRAINT payment_search_pages_candidates_check",
            "UPDATE payment_search_pages SET candidates=jsonb_build_array(repeat('x',8193))",
        ),
    ] {
        let mut tx = pool.begin().await.expect("isolated corruption");
        tx.execute("ALTER TABLE payment_search_pages DISABLE TRIGGER USER")
            .await
            .expect("fixture");
        sqlx::query(drop_sql)
            .execute(&mut *tx)
            .await
            .expect("fixture only");
        sqlx::query(update_sql)
            .execute(&mut *tx)
            .await
            .expect("oversized import");
        let (frozen, at) =
            crate::payment_attempts::evidence::context(&mut tx, &f.snapshot().binding.id, false)
                .await
                .expect("original");
        assert_eq!(
            crate::payment_attempts::searches::rows::load(
                &mut tx,
                &frozen,
                at,
                None,
                Some(&first.id)
            )
            .await
            .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        tx.rollback().await.expect("restore");
    }
    pool.close().await;
    assert_eq!(
        read_recorded(&pool, &f.snapshot().binding.id, "first")
            .await
            .err(),
        Some(PaymentStoreError::Database)
    );
    assert_eq!(
        record_page(&pool, &BadPorts, &BadPorts, "first", None, &page)
            .await
            .err(),
        Some(PaymentStoreError::Database)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn coherent_but_unlinked_imported_cursors_and_revision_exhaustion_fail_closed(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let i = ids();
    let first_page = listed(f.snapshot(), None, Some("tr_A"), &[], false).await;
    let first = record_page(&pool, time.as_ref(), &i, "first", None, &first_page)
        .await
        .expect("first")
        .snapshot;
    let second = record_page(
        &pool,
        time.as_ref(),
        &i,
        "second",
        Some(&first.id),
        &listed(f.snapshot(), Some("tr_A"), None, &[], false).await,
    )
    .await
    .expect("second")
    .snapshot;
    let mut tx = pool.begin().await.expect("coherent header/snapshot damage");
    tx.execute("ALTER TABLE payment_search_pages DISABLE TRIGGER USER")
        .await
        .expect("fixture only");
    tx.execute("UPDATE payment_search_pages SET from_cursor='tr_WRONG',snapshot=jsonb_set(snapshot,'{intent,from}','\"tr_WRONG\"') WHERE effect_key='second'").await.expect("linked-looking corrupt receipt");
    let (frozen, at) =
        crate::payment_attempts::evidence::context(&mut tx, &f.snapshot().binding.id, false)
            .await
            .expect("original");
    assert_eq!(
        crate::payment_attempts::searches::rows::load(&mut tx, &frozen, at, None, Some(&second.id))
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    tx.rollback().await.expect("restore");
    // Exhaustion is an operator-visible stop, never wraparound or another create.
    pool.execute("ALTER TABLE payment_search_pages DISABLE TRIGGER USER; UPDATE payment_search_pages SET revision=9007199254740991,snapshot=jsonb_set(snapshot,'{revision}','9007199254740991') WHERE effect_key='second'; ALTER TABLE payment_search_pages ENABLE TRIGGER USER").await.expect("synthetic limit fixture");
    assert_eq!(
        record_page(&pool, time.as_ref(), &i, "after-limit", None, &first_page)
            .await
            .err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 5, 1, 1, 2)
    );
}
