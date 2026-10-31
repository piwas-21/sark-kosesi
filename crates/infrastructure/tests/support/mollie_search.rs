//! Fixed synthetic list/cursor/identity oracles. No provider resource is accessed.
use super::*;
fn page(payments: Vec<Value>, next: Value) -> Value {
    json!({"count":payments.len(),"_embedded":{"payments":payments},"_links":{"next":next}})
}
fn next(from: &str) -> Value {
    let mut u = url::Url::parse("https://api.mollie.com/v2/payments").expect("fixed API");
    u.query_pairs_mut()
        .append_pair("from", from)
        .append_pair("limit", "10");
    json!({"href":u.as_str(),"type":"application/hal+json"})
}
fn decode(v: &Value, attempt: &AttemptSnapshot) -> Result<SearchPage, MollieError> {
    search::decode(&serde_json::to_vec(v).expect("JSON"), attempt, None)
}
#[tokio::test]
async fn list_provides_only_hints_and_needs_separate_exact_authenticated_fetch() {
    let (_, f) = captured(38245);
    let p = payment(f.snapshot());
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, page(vec![p.clone()], next("tr_NEXT"))),
        Reply::Json(200, p),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified profile");
    let result = client
        .search_page(f.snapshot(), None)
        .await
        .expect("bounded page");
    assert_eq!(
        server.count(),
        2,
        "no automatic pagination or payment fetch"
    );
    assert_eq!(result.candidates().len(), 1);
    assert_eq!(
        result.candidates()[0].provider_id(),
        "tr_SYNTHETIC_NotARealPayment"
    );
    assert_eq!(result.next_cursor().expect("next").provider_id(), "tr_NEXT");
    server.inspect(|requests| {
        let r = &requests[1];
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/v2/payments?limit=10&sort=desc");
        assert!(r.body.is_empty());
        assert!(r.header("Idempotency-Key").is_none());
        assert_eq!(
            r.header("Authorization"),
            Some(format!("Bearer {}", raw_key()).as_str())
        );
    });
    let fetched = client
        .fetch(f.snapshot(), result.candidates()[0].provider_id())
        .await
        .expect("separate fetch");
    assert!(matches!(fetched.payment().status, PaymentStatus::Paid));
    assert_eq!(server.count(), 3);
}
#[tokio::test]
async fn provider_next_link_becomes_only_an_encoded_cursor_on_the_fixed_endpoint() {
    let (_, f) = captured(38245);
    let cursor = "tr_next/a?x=1&y=✓";
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, page(vec![], next(cursor))),
        Reply::Json(200, page(vec![], Value::Null)),
    ]);
    let client = local(&server).verify_profile().await.expect("profile");
    let first = client
        .search_page(f.snapshot(), None)
        .await
        .expect("first page");
    let last = client
        .search_page(f.snapshot(), first.next_cursor())
        .await
        .expect("explicit second page");
    assert!(last.candidates().is_empty());
    assert!(last.next_cursor().is_none());
    server.inspect(|r| {
        assert_eq!(
            r[2].path,
            "/v2/payments?limit=10&sort=desc&from=tr_next%2Fa%3Fx%3D1%26y%3D%E2%9C%93"
        )
    });
}
#[test]
fn arbitrary_unrelated_metadata_and_absent_results_do_not_become_payment_authority() {
    let (_, f) = captured(38245);
    for metadata in [
        Value::Null,
        json!(3),
        json!([1, 2]),
        json!("external"),
        json!({"source":"other"}),
        json!({"order_id":3,"attempt_id":[]}),
    ] {
        let unrelated = json!({"resource":"payment","id":"tr_UNRELATED","metadata":metadata,"mode":"live","amount":{"currency":"USD","value":"1.00"},"status":"unknown"});
        let result =
            decode(&page(vec![unrelated], Value::Null), f.snapshot()).expect("unrelated metadata");
        assert!(result.candidates().is_empty());
        assert!(result.next_cursor().is_none());
    }
    let unrelated = json!({"resource":"payment","id":"tr_WITHOUT_METADATA"});
    assert!(
        decode(&page(vec![unrelated], Value::Null), f.snapshot())
            .expect("missing metadata")
            .candidates()
            .is_empty()
    );
    assert!(
        decode(&page(vec![], Value::Null), f.snapshot())
            .expect("empty page")
            .candidates()
            .is_empty()
    );
}
#[test]
fn either_matching_identifier_requires_full_exact_payment_binding_without_partial_results() {
    let (_, f) = captured(38245);
    for (pointer, value) in [
        ("/metadata/order_id", json!("f".repeat(32))),
        ("/metadata/attempt_id", json!("f".repeat(32))),
        ("/metadata/schema_version", json!(2)),
        ("/mode", json!("live")),
        ("/profileId", json!("pfl_OTHER")),
        ("/amount/value", json!("382.44")),
        ("/sequenceType", json!("recurring")),
        ("/status", json!("unknown")),
    ] {
        let mut bad = payment(f.snapshot());
        bad["id"] = "tr_DISTINCT_BAD_CANDIDATE".into();
        *bad.pointer_mut(pointer).expect("known field") = value;
        assert!(
            decode(
                &page(vec![payment(f.snapshot()), bad], Value::Null),
                f.snapshot()
            )
            .is_err(),
            "{pointer}"
        );
    }
    let mut extra = payment(f.snapshot());
    extra["metadata"]["role"] = "admin".into();
    assert!(decode(&page(vec![extra], Value::Null), f.snapshot()).is_err());
}
#[test]
fn multiple_exact_identity_candidates_remain_visible_for_separate_fetches() {
    let (_, f) = captured(38245);
    let first = payment(f.snapshot());
    let mut second = first.clone();
    second["id"] = "tr_SECOND_DISTINCT".into();
    let result =
        decode(&page(vec![first, second], Value::Null), f.snapshot()).expect("both identities");
    assert_eq!(result.candidates().len(), 2);
    assert_eq!(result.candidates()[1].provider_id(), "tr_SECOND_DISTINCT");
}
#[test]
fn malformed_page_counts_shapes_and_payment_identity_fail_closed() {
    let (_, f) = captured(38245);
    let valid = page(vec![payment(f.snapshot())], Value::Null);
    for v in [
        json!([]),
        json!(null),
        json!({}),
        json!({"count":0,"_embedded":{"payments":[]}}),
    ] {
        assert!(decode(&v, f.snapshot()).is_err());
    }
    for (pointer, value) in [
        ("/count", json!(0)),
        ("/count", json!(-1)),
        ("/count", json!(1.5)),
        ("/count", json!("1")),
        ("/_embedded", json!([])),
        ("/_embedded/payments", json!({})),
        ("/_links", json!([])),
        ("/_links/next", json!(false)),
    ] {
        let mut v = valid.clone();
        *v.pointer_mut(pointer).expect("field") = value;
        assert!(decode(&v, f.snapshot()).is_err(), "{pointer}");
    }
    let many = (0..11)
        .map(|i| json!({"resource":"payment","id":format!("tr_{i}")}))
        .collect();
    assert!(decode(&page(many, Value::Null), f.snapshot()).is_err());
    for p in [
        json!([]),
        json!({"resource":"refund","id":"tr_X"}),
        json!({"resource":"payment","id":"bad"}),
    ] {
        assert!(decode(&page(vec![p], Value::Null), f.snapshot()).is_err());
    }
    let repeated = json!({"resource":"payment","id":"tr_REPEATED"});
    assert!(
        decode(
            &page(vec![repeated.clone(), repeated], Value::Null),
            f.snapshot()
        )
        .is_err()
    );
    let ten = (0..10)
        .map(|i| json!({"resource":"payment","id":format!("tr_{i}")}))
        .collect();
    assert!(decode(&page(ten, Value::Null), f.snapshot()).is_ok());
}
#[test]
fn untrusted_continuation_urls_cannot_choose_origin_path_parameters_or_replay_current_offset() {
    let (_, f) = captured(38245);
    for href in [
        "not a URL",
        "http://api.mollie.com/v2/payments?from=tr_X&limit=10",
        "https://evil.example.test/v2/payments?from=tr_X&limit=10",
        "https://api.mollie.com.evil.test/v2/payments?from=tr_X&limit=10",
        "https://API.MOLLIE.COM/v2/payments?from=tr_X&limit=10",
        "https://api.mollie.com:443/v2/payments?from=tr_X&limit=10",
        "https://api.mollie.com:444/v2/payments?from=tr_X&limit=10",
        "https://user@api.mollie.com/v2/payments?from=tr_X&limit=10",
        "https://user:pass@api.mollie.com/v2/payments?from=tr_X&limit=10",
        "https://:pass@api.mollie.com/v2/payments?from=tr_X&limit=10",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=10#fragment",
        "https://api.mollie.com/v2/refunds?from=tr_X&limit=10",
        "https://api.mollie.com/v2/payments?limit=10",
        "https://api.mollie.com/v2/payments?from=tr_X",
        "https://api.mollie.com/v2/payments?from=bad&limit=10",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=11",
        "https://api.mollie.com/v2/payments?from=tr_X&from=tr_Y&limit=10",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=10&limit=10",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=10&sort=asc",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=10&sort=desc&sort=desc",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=10&testmode=true",
        "https://api.mollie.com/v2/payments?from=tr_X&limit=10&profileId=pfl_X",
    ] {
        assert!(
            decode(&page(vec![], json!({"href":href})), f.snapshot()).is_err(),
            "{href}"
        );
    }
    let from = SearchCursor::capture("tr_SAME").expect("cursor");
    assert!(
        search::decode(
            &serde_json::to_vec(&page(vec![], next("tr_SAME"))).expect("JSON"),
            f.snapshot(),
            Some(&from)
        )
        .is_err()
    );
    let advancing = page(vec![], next("tr_DIFFERENT"));
    assert!(
        search::decode(
            &serde_json::to_vec(&advancing).expect("JSON"),
            f.snapshot(),
            Some(&from)
        )
        .is_ok()
    );
    let huge = format!(
        "https://api.mollie.com/v2/payments?from=tr_{}&limit=10",
        "x".repeat(2050)
    );
    assert!(decode(&page(vec![], json!({"href":huge})), f.snapshot()).is_err());
    let sorted = json!({"href":"https://api.mollie.com/v2/payments?from=tr_X&sort=desc&limit=10"});
    assert_eq!(
        decode(&page(vec![], sorted), f.snapshot())
            .expect("desc allowed")
            .next_cursor()
            .expect("next")
            .provider_id(),
        "tr_X"
    );
    for bad in [
        "",
        "tr_",
        "tr_\n",
        "bad",
        &format!("tr_{}", "x".repeat(254)),
    ] {
        assert!(SearchCursor::capture(bad).is_err());
    }
}
#[test]
fn raw_list_entries_preserve_duplicate_critical_field_rejection() {
    let (_, f) = captured(38245);
    let mut p = payment(f.snapshot());
    p.as_object_mut().expect("object").remove("_links");
    let valid = page(vec![p], next("tr_NEXT")).to_string();
    for (old, new) in [
        ("\"count\":1", "\"count\":1,\"count\":1"),
        ("\"_embedded\":{", "\"_embedded\":{},\"_embedded\":{"),
        ("\"payments\":[", "\"payments\":[],\"payments\":["),
        ("\"next\":{", "\"next\":null,\"next\":{"),
        ("\"href\":", "\"href\":\"https://evil.test\",\"href\":"),
        (
            "\"resource\":\"payment\"",
            "\"resource\":\"payment\",\"resource\":\"payment\"",
        ),
        (
            "\"id\":\"tr_SYNTHETIC_NotARealPayment\"",
            "\"id\":\"tr_OTHER\",\"id\":\"tr_SYNTHETIC_NotARealPayment\"",
        ),
        ("\"metadata\":{", "\"metadata\":{},\"metadata\":{"),
        (
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
        ),
        ("\"order_id\":", "\"order_id\":\"other\",\"order_id\":"),
        (
            "\"attempt_id\":",
            "\"attempt_id\":\"other\",\"attempt_id\":",
        ),
        (
            "\"value\":\"382.45\"",
            "\"value\":\"382.45\",\"value\":\"382.45\"",
        ),
    ] {
        assert!(valid.contains(old), "fixture {old}");
        let bad = valid.replacen(old, new, 1);
        assert!(
            search::decode(bad.as_bytes(), f.snapshot(), None).is_err(),
            "{old}"
        );
    }
}
#[tokio::test]
async fn rotated_same_profile_search_is_read_only_and_wrong_attempt_profile_is_rejected_before_io()
{
    let (_, f) = captured(38245);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, page(vec![payment(f.snapshot())], Value::Null)),
    ]);
    let raw = format!("test_{}", "rotated".repeat(5));
    let key = TestCredential::capture(&raw, "pfl_SYNTHETIC_NotARealProfile").expect("rotated");
    let c = config(&key);
    let client = TestClient::local(key, c, server.base.clone(), Duration::from_secs(2))
        .expect("local")
        .verify_profile()
        .await
        .expect("profile");
    let mut wrong = f.snapshot().clone();
    wrong.binding.profile.id = "pfl_OTHER".into();
    assert!(matches!(
        client.search_page(&wrong, None).await,
        Err(MollieError::PaymentMismatch)
    ));
    wrong = f.snapshot().clone();
    wrong.schema_version = 2;
    assert!(matches!(
        client.search_page(&wrong, None).await,
        Err(MollieError::PaymentMismatch)
    ));
    assert_eq!(server.count(), 1);
    assert_eq!(
        client
            .search_page(f.snapshot(), None)
            .await
            .expect("historical same profile")
            .candidates()
            .len(),
        1
    );
    server.inspect(|r| {
        assert_eq!(
            r[1].header("Authorization"),
            Some(format!("Bearer {raw}").as_str())
        )
    });
    assert_eq!(
        server.count(),
        2,
        "search never posts or automatically fetches"
    );
}
