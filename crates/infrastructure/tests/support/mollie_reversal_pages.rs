//! Isolated PostgreSQL and authenticated loopback page custody controls.
use super::*;
use crate::mollie::{ReversalCursor, ReversalPage};
use crate::payment_attempts::reversal_pages::{lookup, read_page, record, resume};
use sqlx::Executor;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";

fn collection(kind: ReversalKind) -> &'static str {
    match kind {
        ReversalKind::Refund => "refunds",
        ReversalKind::Chargeback => "chargebacks",
    }
}
fn page(payment_id: &str, kind: ReversalKind, ids: &[&str], next: Option<&str>) -> Value {
    let children: Vec<_> = ids
        .iter()
        .map(|id| match kind {
            ReversalKind::Refund => {
                json!({"resource":"refund","id":id,"paymentId":payment_id,"mode":"test"})
            }
            ReversalKind::Chargeback => {
                json!({"resource":"chargeback","id":id,"paymentId":payment_id})
            }
        })
        .collect();
    let next = next.map_or(Value::Null, |offset| {
        let mut url = url::Url::parse(&format!(
            "https://api.mollie.com/v2/payments/{payment_id}/{}",
            collection(kind)
        ))
        .expect("fixed endpoint");
        url.query_pairs_mut()
            .append_pair("from", offset)
            .append_pair("limit", "10");
        json!({"href":url.as_str()})
    });
    json!({"count":children.len(),"_embedded":{(collection(kind)):children},"_links":{"next":next}})
}
async fn listed(
    attempt: &AttemptSnapshot,
    kind: ReversalKind,
    cursor: Option<&ReversalCursor>,
    next: Option<&str>,
    candidates: &[&str],
    rotated: bool,
) -> ReversalPage {
    listed_for(attempt, PAYMENT, kind, cursor, next, candidates, rotated).await
}
async fn listed_for(
    attempt: &AttemptSnapshot,
    payment_id: &str,
    kind: ReversalKind,
    cursor: Option<&ReversalCursor>,
    next: Option<&str>,
    candidates: &[&str],
    rotated: bool,
) -> ReversalPage {
    let key_text = if rotated {
        format!("test_{}", "rotated".repeat(5))
    } else {
        raw_key()
    };
    let key =
        TestCredential::capture(&key_text, "pfl_SYNTHETIC_NotARealProfile").expect("synthetic key");
    let mut parent = payment(attempt);
    parent["id"] = json!(payment_id);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, parent),
        Reply::Json(200, page(payment_id, kind, candidates, next)),
    ]);
    let client = TestClient::local(
        key,
        config(&credential_for(&key_text)),
        server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("test profile")
    .into_reconciliation();
    let result = client
        .reversal_page(attempt, payment_id, kind, cursor)
        .await
        .expect("authenticated paid parent and page");
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {key_text}").as_str())
            );
            assert!(request.body.is_empty());
        }
        assert_eq!(requests[1].path, format!("/v2/payments/{payment_id}"));
        assert!(requests[2].path.starts_with(&format!(
            "/v2/payments/{payment_id}/{}?limit=10",
            collection(kind)
        )));
    });
    result
}
fn credential_for(value: &str) -> TestCredential {
    TestCredential::capture(value, "pfl_SYNTHETIC_NotARealProfile").expect("synthetic key")
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    let pages = sqlx::query_scalar("SELECT count(*) FROM payment_reversal_pages")
        .fetch_one(pool)
        .await
        .expect("pages");
    let events = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_outbox WHERE kind='payment.reversal_page_recorded.v1'",
    )
    .fetch_one(pool)
    .await
    .expect("events");
    (pages, events)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authenticated_pages_survive_restart_and_resume_once_without_financial_effect(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let before = crate::payment_test_fixture::counts(&pool).await;
    let ids = observation_cases::ids();
    let first_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_NEXT"),
        &["re_ONE", "re_TWO"],
        false,
    )
    .await;
    let first = record(&pool, time.as_ref(), &ids, "page-root", None, &first_page)
        .await
        .expect("root");
    assert!(!first.replayed);
    assert_eq!(first.order_id, frozen.snapshot().binding.order_id);
    assert_eq!(first.attempt_id, *attempt);
    assert_eq!(first.payment_id, PAYMENT);
    assert_eq!(first.kind, ReversalKind::Refund);
    assert_eq!(first.page_number, 1);
    assert_eq!(first.candidates, vec!["re_ONE", "re_TWO"]);
    assert_eq!(first.recorded_at, 1500);
    assert_eq!(
        first.credential_fingerprint,
        "78a0e854453c8e82b9c97187636308791541cdbbcf28965b47a6ee3e33af97ef"
    );
    assert_eq!(first.from_cursor, None);
    assert_eq!(first.next_cursor.as_deref(), Some("re_NEXT"));
    let recovered = lookup(&pool, attempt, "page-root")
        .await
        .expect("lookup")
        .expect("saved");
    assert_eq!(recovered.id, first.id);
    assert_eq!(
        read_page(&pool, attempt, &first.id).await.expect("read").id,
        first.id
    );
    let replay = record(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        "page-root",
        None,
        &first_page,
    )
    .await
    .expect("historical replay");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    let changed = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &[],
        false,
    )
    .await;
    assert!(
        record(&pool, time.as_ref(), &ids, "page-root", None, &changed)
            .await
            .is_err()
    );
    let cursor = resume(&pool, attempt, &first.id)
        .await
        .expect("resume")
        .expect("next page");
    let last_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&cursor),
        None,
        &[],
        false,
    )
    .await;
    let last = record(
        &pool,
        time.as_ref(),
        &ids,
        "page-end",
        Some(&first.id),
        &last_page,
    )
    .await
    .expect("terminal");
    assert_eq!(last.page_number, 2);
    assert_eq!(last.scan_id, first.scan_id);
    assert_eq!(last.previous_page_id.as_deref(), Some(first.id.as_str()));
    assert_eq!(last.from_cursor.as_deref(), Some("re_NEXT"));
    assert!(last.candidates.is_empty() && last.next_cursor.is_none());
    assert!(
        resume(&pool, attempt, &first.id)
            .await
            .expect("followed")
            .is_none()
    );
    assert!(
        resume(&pool, attempt, &last.id)
            .await
            .expect("ended")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (2, 2));
    let after = crate::payment_test_fixture::counts(&pool).await;
    assert_eq!(
        after,
        (before.0, before.1 + 2, before.2, before.3, before.4)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_reversal_observations")
            .fetch_one(&pool)
            .await
            .expect("no child observations"),
        0
    );
    let payloads: Vec<Value> = sqlx::query_scalar("SELECT payload FROM commerce_outbox WHERE kind='payment.reversal_page_recorded.v1' ORDER BY created_at,id").fetch_all(&pool).await.expect("event payloads");
    for payload in payloads {
        let object = payload.as_object().expect("closed event");
        assert_eq!(object.len(), 5);
        assert_eq!(object.get("schema_version"), Some(&json!(1)));
        assert_eq!(object.get("order_id"), Some(&json!(first.order_id)));
        assert_eq!(object.get("attempt_id"), Some(&json!(attempt)));
        let digest = object
            .get("page_sha256")
            .and_then(Value::as_str)
            .expect("digest");
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn one_successor_wins_and_exact_retry_remains_available(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let first_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_NEXT"),
        &[],
        false,
    )
    .await;
    let first = record(&pool, time.as_ref(), &ids, "root", None, &first_page)
        .await
        .expect("root");
    let cursor = resume(&pool, &first.attempt_id, &first.id)
        .await
        .expect("resume")
        .expect("next");
    let next = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&cursor),
        None,
        &["re_ONE"],
        false,
    )
    .await;
    let (a, b) = tokio::join!(
        record(&pool, time.as_ref(), &ids, "next-a", Some(&first.id), &next),
        record(&pool, time.as_ref(), &ids, "next-b", Some(&first.id), &next),
    );
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "one competing effect may claim a successor"
    );
    let (winner, effect) = if let Ok(a) = a {
        (a, "next-a")
    } else {
        (b.expect("other winner"), "next-b")
    };
    let replay = record(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        effect,
        Some(&first.id),
        &next,
    )
    .await
    .expect("replay after follow");
    assert!(replay.replayed);
    assert_eq!(replay.id, winner.id);
    assert!(
        resume(&pool, &first.attempt_id, &first.id)
            .await
            .expect("followed")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (2, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_page_or_event_rolls_back_and_healthy_control_succeeds(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &[],
        false,
    )
    .await;
    pool.execute("CREATE FUNCTION test_suppress_reversal_page() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").await.expect("fault function");
    for (create, remove, effect) in [
        (
            "CREATE TRIGGER suppress_page BEFORE INSERT ON payment_reversal_pages FOR EACH ROW EXECUTE FUNCTION test_suppress_reversal_page()",
            "DROP TRIGGER suppress_page ON payment_reversal_pages",
            "suppressed-page",
        ),
        (
            "CREATE TRIGGER suppress_page_event BEFORE INSERT ON commerce_outbox FOR EACH ROW WHEN (NEW.kind='payment.reversal_page_recorded.v1') EXECUTE FUNCTION test_suppress_reversal_page()",
            "DROP TRIGGER suppress_page_event ON commerce_outbox",
            "suppressed-event",
        ),
    ] {
        pool.execute(create).await.expect("install fault");
        assert!(
            record(&pool, time.as_ref(), &ids, effect, None, &page)
                .await
                .is_err()
        );
        pool.execute(remove).await.expect("remove fault");
        assert!(
            lookup(&pool, &frozen.snapshot().binding.id, effect)
                .await
                .expect("lookup")
                .is_none()
        );
        assert_eq!(counts(&pool).await, (0, 0));
    }
    record(&pool, time.as_ref(), &ids, "healthy", None, &page)
        .await
        .expect("positive control");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn valid_looking_historical_candidate_tampering_breaks_digest_and_descendant_reads(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let first_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_NEXT"),
        &["re_ONE"],
        false,
    )
    .await;
    let first = record(&pool, time.as_ref(), &ids, "root", None, &first_page)
        .await
        .expect("root");
    let cursor = resume(&pool, &first.attempt_id, &first.id)
        .await
        .expect("resume")
        .expect("cursor");
    let next_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&cursor),
        None,
        &[],
        false,
    )
    .await;
    let next = record(
        &pool,
        time.as_ref(),
        &ids,
        "next",
        Some(&first.id),
        &next_page,
    )
    .await
    .expect("next");
    assert!(read_page(&pool, &first.attempt_id, &next.id).await.is_ok());
    pool.execute("ALTER TABLE payment_reversal_pages DISABLE TRIGGER USER; UPDATE payment_reversal_pages SET candidates=ARRAY['re_FORGED'] WHERE effect_key='root'; ALTER TABLE payment_reversal_pages ENABLE TRIGGER USER").await.expect("controlled historical tamper");
    assert!(lookup(&pool, &first.attempt_id, "root").await.is_err());
    assert!(read_page(&pool, &first.attempt_id, &next.id).await.is_err());
    assert!(resume(&pool, &first.attempt_id, &next.id).await.is_err());
    assert!(
        record(
            &pool,
            &observation_cases::BadPorts,
            &observation_cases::BadPorts,
            "root",
            None,
            &first_page
        )
        .await
        .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn restored_cursor_rejects_changed_attempt_payment_collection_and_key_before_network(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let first_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_NEXT"),
        &[],
        false,
    )
    .await;
    let first = record(&pool, time.as_ref(), &ids, "root", None, &first_page)
        .await
        .expect("root");
    let cursor = resume(&pool, &first.attempt_id, &first.id)
        .await
        .expect("resume")
        .expect("cursor");
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let mut foreign = frozen.snapshot().clone();
    foreign.binding.id = "f".repeat(32);
    assert!(
        client
            .reversal_page(&foreign, PAYMENT, ReversalKind::Refund, Some(&cursor))
            .await
            .is_err()
    );
    assert!(
        client
            .reversal_page(
                frozen.snapshot(),
                "tr_OTHER",
                ReversalKind::Refund,
                Some(&cursor)
            )
            .await
            .is_err()
    );
    assert!(
        client
            .reversal_page(
                frozen.snapshot(),
                PAYMENT,
                ReversalKind::Chargeback,
                Some(&cursor)
            )
            .await
            .is_err()
    );
    assert_eq!(server.count(), 1);
    let rotated_key = credential_for(&format!("test_{}", "rotated".repeat(5)));
    let rotated_config = config(&rotated_key);
    let rotated_server = Server::start(vec![profile()]);
    let rotated = TestClient::local(
        rotated_key,
        rotated_config,
        rotated_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("profile")
    .into_reconciliation();
    assert!(
        rotated
            .reversal_page(
                frozen.snapshot(),
                PAYMENT,
                ReversalKind::Refund,
                Some(&cursor)
            )
            .await
            .is_err()
    );
    assert_eq!(rotated_server.count(), 1);
    let rotated_root = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &[],
        true,
    )
    .await;
    assert!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "wrong-parent-key",
            Some(&first.id),
            &rotated_root
        )
        .await
        .is_err()
    );
    let rotated_saved = record(
        &pool,
        time.as_ref(),
        &ids,
        "rotated-root",
        None,
        &rotated_root,
    )
    .await
    .expect("new scan with rotated key");
    assert_ne!(
        first.credential_fingerprint,
        rotated_saved.credential_fingerprint
    );
    assert_ne!(first.scan_id, rotated_saved.scan_id);
    let chargeback = listed(
        frozen.snapshot(),
        ReversalKind::Chargeback,
        None,
        None,
        &["chb_ONE"],
        false,
    )
    .await;
    assert!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "wrong-collection",
            Some(&first.id),
            &chargeback
        )
        .await
        .is_err()
    );
    let chargeback_saved = record(
        &pool,
        time.as_ref(),
        &ids,
        "chargeback-root",
        None,
        &chargeback,
    )
    .await
    .expect("separate collection root");
    assert_eq!(chargeback_saved.kind, ReversalKind::Chargeback);
    assert_eq!(counts(&pool).await, (3, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn thirty_two_page_scan_stops_even_with_a_provider_next_link(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let root = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_1"),
        &[],
        false,
    )
    .await;
    let mut saved = record(&pool, time.as_ref(), &ids, "page-1", None, &root)
        .await
        .expect("root");
    for n in 2..=32 {
        let cursor = resume(&pool, &saved.attempt_id, &saved.id)
            .await
            .expect("valid prefix")
            .expect("continuation under cap");
        let next = format!("re_{n}");
        let verified = listed(
            frozen.snapshot(),
            ReversalKind::Refund,
            Some(&cursor),
            Some(&next),
            &[],
            false,
        )
        .await;
        saved = record(
            &pool,
            time.as_ref(),
            &ids,
            &format!("page-{n}"),
            Some(&saved.id),
            &verified,
        )
        .await
        .expect("bounded page");
        assert_eq!(saved.page_number, n);
    }
    assert_eq!(saved.next_cursor.as_deref(), Some("re_32"));
    assert!(
        resume(&pool, &saved.attempt_id, &saved.id)
            .await
            .expect("capped")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (32, 32));
    assert!(
        record(
            &pool,
            &observation_cases::BadPorts,
            &observation_cases::BadPorts,
            "page-1",
            None,
            &root
        )
        .await
        .expect("historical replay at cap")
        .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn attempt_capacity_stops_new_pages_but_preserves_exact_replay(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &[],
        false,
    )
    .await;
    let open_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_UNFOLLOWED"),
        &[],
        false,
    )
    .await;
    let first = record(&pool, time.as_ref(), &ids, "scan-0", None, &open_page)
        .await
        .expect("resumable root");
    assert!(
        resume(&pool, &first.attempt_id, &first.id)
            .await
            .expect("under capacity")
            .is_some()
    );
    for n in 1..127 {
        record(
            &pool,
            time.as_ref(),
            &ids,
            &format!("scan-{n}"),
            None,
            &page,
        )
        .await
        .expect("root below attempt cap");
    }
    let (a, b) = tokio::join!(
        record(&pool, time.as_ref(), &ids, "final-a", None, &page),
        record(&pool, time.as_ref(), &ids, "final-b", None, &page),
    );
    assert_ne!(a.is_ok(), b.is_ok(), "only one final slot is available");
    let rejected = if a.is_ok() { "final-b" } else { "final-a" };
    assert!(
        lookup(&pool, &frozen.snapshot().binding.id, rejected)
            .await
            .expect("losing effect")
            .is_none()
    );
    assert!(
        record(&pool, time.as_ref(), &ids, "over-cap", None, &page)
            .await
            .is_err()
    );
    assert!(
        lookup(&pool, &frozen.snapshot().binding.id, "over-cap")
            .await
            .expect("no cap effect")
            .is_none()
    );
    assert!(
        resume(&pool, &first.attempt_id, &first.id)
            .await
            .expect("attempt capacity")
            .is_none()
    );
    let replay = record(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        "scan-0",
        None,
        &open_page,
    )
    .await
    .expect("historical replay at cap");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    assert_eq!(counts(&pool).await, (128, 128));
}

async fn authenticated_child(
    attempt: &AttemptSnapshot,
    payment_id: &str,
    kind: ReversalKind,
    offset: &str,
    rotated: bool,
) -> ReversalPage {
    let root = listed_for(attempt, payment_id, kind, None, Some(offset), &[], rotated).await;
    listed_for(
        attempt,
        payment_id,
        kind,
        root.next_cursor(),
        None,
        &[],
        rotated,
    )
    .await
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn nonadjacent_cursor_cycle_is_rejected_by_writer_and_native_guard(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let root_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_A"),
        &[],
        false,
    )
    .await;
    let root = record(&pool, time.as_ref(), &ids, "cycle-root", None, &root_page)
        .await
        .expect("root");
    let a = resume(&pool, &root.attempt_id, &root.id)
        .await
        .expect("root resume")
        .expect("A");
    let middle_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&a),
        Some("re_B"),
        &[],
        false,
    )
    .await;
    let middle = record(
        &pool,
        time.as_ref(),
        &ids,
        "cycle-middle",
        Some(&root.id),
        &middle_page,
    )
    .await
    .expect("middle");
    let b = resume(&pool, &root.attempt_id, &middle.id)
        .await
        .expect("middle resume")
        .expect("B");
    let cycle = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&b),
        Some("re_A"),
        &[],
        false,
    )
    .await;
    assert!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "cycle-rejected",
            Some(&middle.id),
            &cycle
        )
        .await
        .is_err()
    );
    assert!(
        lookup(&pool, &root.attempt_id, "cycle-rejected")
            .await
            .expect("no receipt")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (2, 2));

    let mut tx = pool.begin().await.expect("native guard");
    let inserted = sqlx::query("INSERT INTO payment_reversal_pages(id,order_id,attempt_id,effect_key,payment_id,reversal_kind,credential_fingerprint,scan_id,page_number,previous_page_id,from_cursor,next_cursor,candidates,recorded_at,event_id) VALUES($1,$2,$3,'native-cycle',$4,'refund',$5,$6,3,$7,'re_B','re_A',ARRAY[]::text[],$8,$9)")
        .bind("f".repeat(32)).bind(&root.order_id).bind(&root.attempt_id).bind(PAYMENT)
        .bind(&root.credential_fingerprint).bind(&root.scan_id).bind(&middle.id).bind(root.recorded_at).bind("e".repeat(32))
        .execute(&mut *tx).await;
    assert!(
        inserted.is_err(),
        "native ancestry guard rejects a cycle before an event can be written"
    );
    tx.rollback().await.expect("discard rejected native write");
    assert_eq!(counts(&pool).await, (2, 2));
    let healthy_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&b),
        None,
        &["re_OK"],
        false,
    )
    .await;
    let healthy = record(
        &pool,
        time.as_ref(),
        &ids,
        "cycle-healthy",
        Some(&middle.id),
        &healthy_page,
    )
    .await
    .expect("healthy third page");
    assert_eq!(healthy.page_number, 3);
    assert_eq!(counts(&pool).await, (3, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replay_and_fresh_continuation_require_the_full_authenticated_scope(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let root_page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        Some("re_A"),
        &[],
        false,
    )
    .await;
    let root = record(&pool, time.as_ref(), &ids, "scope-root", None, &root_page)
        .await
        .expect("root");
    let spare = record(&pool, time.as_ref(), &ids, "scope-spare", None, &root_page)
        .await
        .expect("separate open scan");
    let a = resume(&pool, &root.attempt_id, &root.id)
        .await
        .expect("resume")
        .expect("A");
    let exact = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        Some(&a),
        None,
        &[],
        false,
    )
    .await;
    let receipt = record(
        &pool,
        time.as_ref(),
        &ids,
        "scope-child",
        Some(&root.id),
        &exact,
    )
    .await
    .expect("child");
    let alternate_payment = authenticated_child(
        frozen.snapshot(),
        "tr_ALTERNATE",
        ReversalKind::Refund,
        "re_A",
        false,
    )
    .await;
    let alternate_kind = authenticated_child(
        frozen.snapshot(),
        PAYMENT,
        ReversalKind::Chargeback,
        "chb_A",
        false,
    )
    .await;
    let alternate_key = authenticated_child(
        frozen.snapshot(),
        PAYMENT,
        ReversalKind::Refund,
        "re_A",
        true,
    )
    .await;
    let alternate_offset = authenticated_child(
        frozen.snapshot(),
        PAYMENT,
        ReversalKind::Refund,
        "re_OTHER",
        false,
    )
    .await;
    assert!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "scope-child",
            Some(&spare.id),
            &exact
        )
        .await
        .is_err(),
        "predecessor is part of replay intent"
    );
    for different in [
        &alternate_payment,
        &alternate_kind,
        &alternate_key,
        &alternate_offset,
    ] {
        assert!(
            record(
                &pool,
                time.as_ref(),
                &ids,
                "scope-child",
                Some(&root.id),
                different
            )
            .await
            .is_err(),
            "authenticated intent changed"
        );
        assert!(
            record(
                &pool,
                time.as_ref(),
                &ids,
                "fresh-wrong-scope",
                Some(&spare.id),
                different
            )
            .await
            .is_err(),
            "open predecessor requires matching authenticated scope"
        );
        assert!(
            lookup(&pool, &root.attempt_id, "fresh-wrong-scope")
                .await
                .expect("no receipt")
                .is_none()
        );
    }
    assert_eq!(counts(&pool).await, (3, 3));
    let replay = record(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        "scope-child",
        Some(&root.id),
        &exact,
    )
    .await
    .expect("exact replay positive control");
    assert!(replay.replayed);
    assert_eq!(replay.id, receipt.id);
    let healthy = record(
        &pool,
        time.as_ref(),
        &ids,
        "scope-spare-child",
        Some(&spare.id),
        &exact,
    )
    .await
    .expect("matching open predecessor positive control");
    assert_eq!(healthy.previous_page_id.as_deref(), Some(spare.id.as_str()));
    assert_eq!(counts(&pool).await, (4, 4));
}

#[path = "mollie_reversal_page_boundaries.rs"]
mod boundaries;
