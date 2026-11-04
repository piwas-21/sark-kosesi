//! Synthetic loopback evidence; no provider calls or reversal authority.
use super::*;
const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";
fn page(kind: ReversalKind, items: Vec<Value>, next: Value) -> Value {
    let collection = match kind {
        ReversalKind::Refund => "refunds",
        ReversalKind::Chargeback => "chargebacks",
    };
    json!({"count":items.len(),"_embedded":{(collection):items},"_links":{"next":next}})
}
fn item(kind: ReversalKind) -> Value {
    match kind {
        ReversalKind::Refund => {
            json!({"resource":"refund","id":"re_first","paymentId":PAYMENT,"mode":"test","status":"untrusted"})
        }
        ReversalKind::Chargeback => {
            json!({"resource":"chargeback","id":"chb_first","paymentId":PAYMENT})
        }
    }
}
fn link(path: &str) -> Value {
    json!({"href":path})
}
async fn client(server: &Server) -> VerifiedReconciliationClient {
    local(server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation()
}
#[tokio::test]
async fn authenticated_paid_parent_precedes_each_bounded_page_and_cursor_is_reconstructed() {
    let (_, frozen) = captured(38245);
    for kind in [ReversalKind::Refund, ReversalKind::Chargeback] {
        let (collection, offset) = match kind {
            ReversalKind::Refund => ("refunds", "re_next/a?x=1"),
            ReversalKind::Chargeback => ("chargebacks", "chb_next/a?x=1"),
        };
        let mut url = url::Url::parse(&format!(
            "https://api.mollie.com/v2/payments/{PAYMENT}/{collection}"
        ))
        .expect("URL");
        url.query_pairs_mut().append_pair("from", offset);
        let server = Server::start(vec![
            profile(),
            Reply::Json(200, payment(frozen.snapshot())),
            Reply::Json(200, page(kind, vec![item(kind)], link(url.as_str()))),
            Reply::Json(200, payment(frozen.snapshot())),
            Reply::Json(200, page(kind, vec![], Value::Null)),
        ]);
        let client = client(&server).await;
        let first = client
            .reversal_page(frozen.snapshot(), PAYMENT, kind, None)
            .await
            .expect("first page");
        assert_eq!(first.candidates().len(), 1);
        let expected = match kind {
            ReversalKind::Refund => "re_first",
            ReversalKind::Chargeback => "chb_first",
        };
        assert_eq!(first.candidates()[0], expected);
        let second = client
            .reversal_page(frozen.snapshot(), PAYMENT, kind, first.next_cursor())
            .await
            .expect("next page");
        assert!(second.candidates().is_empty());
        assert!(second.next_cursor().is_none());
        server.inspect(|r| {
            assert_eq!(r.len(), 5);
            for request in r {
                assert_eq!(request.method, "GET");
                assert!(request.body.is_empty());
                assert_eq!(
                    request.header("Authorization"),
                    Some(format!("Bearer {}", raw_key()).as_str())
                );
            }
            assert_eq!(r[1].path, format!("/v2/payments/{PAYMENT}"));
            assert_eq!(
                r[2].path,
                format!("/v2/payments/{PAYMENT}/{collection}?limit=10")
            );
            assert_eq!(r[3].path, format!("/v2/payments/{PAYMENT}"));
            let mut expected = url::Url::parse(&format!(
                "http://localhost/v2/payments/{PAYMENT}/{collection}"
            ))
            .expect("URL");
            expected
                .query_pairs_mut()
                .append_pair("limit", "10")
                .append_pair("from", offset);
            assert_eq!(
                r[4].path,
                format!("{}?{}", expected.path(), expected.query().expect("query"))
            );
        });
    }
}
#[tokio::test]
async fn mismatched_or_unpaid_parent_never_causes_child_request() {
    let (_, frozen) = captured(38245);
    for (field, value) in [
        ("status", json!("pending")),
        ("profileId", json!("pfl_OTHER")),
        ("mode", json!("live")),
    ] {
        let mut parent = payment(frozen.snapshot());
        parent[field] = value;
        let server = Server::start(vec![profile(), Reply::Json(200, parent)]);
        assert!(
            client(&server)
                .await
                .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
                .await
                .is_err()
        );
        assert_eq!(server.count(), 2);
    }
}
#[tokio::test]
async fn cursors_reject_cross_attempt_payment_kind_and_credential_before_io() {
    let (_, frozen) = captured(38245);
    let url = format!("https://api.mollie.com/v2/payments/{PAYMENT}/refunds?from=re_next&limit=10");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, page(ReversalKind::Refund, vec![], link(&url))),
    ]);
    let client = client(&server).await;
    let page = client
        .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("page");
    let cursor = page.next_cursor();
    let mut other = frozen.snapshot().clone();
    other.binding.id = "f".repeat(32);
    assert!(
        client
            .reversal_page(&other, PAYMENT, ReversalKind::Refund, cursor)
            .await
            .is_err()
    );
    assert!(
        client
            .reversal_page(frozen.snapshot(), "tr_OTHER", ReversalKind::Refund, cursor)
            .await
            .is_err()
    );
    assert!(
        client
            .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Chargeback, cursor)
            .await
            .is_err()
    );
    assert_eq!(server.count(), 3);
    let rotated = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(4)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("credential");
    let rotated_server = Server::start(vec![profile()]);
    let rotated_config = config(&rotated);
    let rotated_client = TestClient::local(
        rotated,
        rotated_config,
        rotated_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("local")
    .verify_profile()
    .await
    .expect("profile")
    .into_reconciliation();
    assert!(
        rotated_client
            .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, cursor)
            .await
            .is_err()
    );
    assert_eq!(rotated_server.count(), 1);
}
#[tokio::test]
async fn invalid_parent_identity_fails_before_a_payment_request() {
    let (_, frozen) = captured(38245);
    let server = Server::start(vec![profile()]);
    assert!(matches!(
        client(&server)
            .await
            .reversal_page(frozen.snapshot(), "tr_", ReversalKind::Refund, None)
            .await,
        Err(MollieError::PaymentMismatch)
    ));
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn hostile_links_and_malformed_pages_fail_closed() {
    let (_, frozen) = captured(38245);
    let base = format!("https://api.mollie.com/v2/payments/{PAYMENT}/refunds");
    let links = [
        format!("https://API.MOLLIE.COM/v2/payments/{PAYMENT}/refunds?from=re_next"),
        format!("https://:password@api.mollie.com/v2/payments/{PAYMENT}/refunds?from=re_next"),
        format!("http://api.mollie.com/v2/payments/{PAYMENT}/refunds?from=re_next"),
        format!("https://api.mollie.com.evil.test/v2/payments/{PAYMENT}/refunds?from=re_next"),
        "https://api.mollie.com/v2/payments/tr_OTHER/refunds?from=re_next".to_owned(),
        format!("https://api.mollie.com/v2/payments/{PAYMENT}/chargebacks?from=re_next"),
        format!("{base}?from=re_next&from=re_second"),
        format!("{base}?from=re_next&limit=11"),
        format!("{base}?from=re_next&testmode=true"),
        format!("{base}?from=chb_wrong"),
        format!("{base}?from=re_next#fragment"),
        format!("{base}?from=re_%FF"),
        format!("{base}?from=re_%XY"),
        format!("{base}?from=re_next&"),
        format!("{base}?from=re_next&limit"),
        format!("{base}?limit=10"),
        format!("{base}?from=re_next&limit=10&limit=10"),
        base.clone(),
        format!("https://user:password@api.mollie.com/v2/payments/{PAYMENT}/refunds?from=re_next"),
        format!("https://api.mollie.com:444/v2/payments/{PAYMENT}/refunds?from=re_next"),
        format!("https://api.mollie.com/v3/payments/{PAYMENT}/refunds?from=re_next"),
        format!("https://api.mollie.com/v2/transfers/{PAYMENT}/refunds?from=re_next"),
        format!("https://api.mollie.com/v2/payments/{PAYMENT}/refunds/extra?from=re_next"),
        format!("{base}?from=re_{}", "a".repeat(2100)),
    ];
    let mut cases: Vec<Value> = links
        .iter()
        .map(|v| page(ReversalKind::Refund, vec![], link(v)))
        .collect();
    let mut wrong_count = page(ReversalKind::Refund, vec![], Value::Null);
    wrong_count["count"] = json!(1);
    cases.push(wrong_count);
    cases.push(page(
        ReversalKind::Refund,
        vec![item(ReversalKind::Refund); 11],
        Value::Null,
    ));
    cases.push(page(
        ReversalKind::Refund,
        vec![item(ReversalKind::Refund), item(ReversalKind::Refund)],
        Value::Null,
    ));
    for (field, value) in [
        ("resource", json!("chargeback")),
        ("paymentId", json!("tr_OTHER")),
        ("id", json!("re_")),
        ("mode", json!("live")),
        ("mode", Value::Null),
    ] {
        let mut bad = item(ReversalKind::Refund);
        bad[field] = value;
        cases.push(page(ReversalKind::Refund, vec![bad], Value::Null));
    }
    let mut missing_mode = item(ReversalKind::Refund);
    missing_mode.as_object_mut().expect("item").remove("mode");
    cases.push(page(ReversalKind::Refund, vec![missing_mode], Value::Null));
    cases.push(json!({"count":0,"_embedded":{"refunds":{}},"_links":{"next":null}}));
    cases.extend([
        json!([]),
        json!({"count":0,"_links":{"next":null}}),
        json!({"count":0,"_embedded":{"refunds":[]}}),
        json!({"count":1,"_embedded":{"refunds":[false]},"_links":{"next":null}}),
        json!({"count":0,"_embedded":{"refunds":[]},"_links":{"next":false}}),
        page(ReversalKind::Refund, vec![], json!({"href":"not a URL"})),
    ]);
    for bad in cases {
        let server = Server::start(vec![
            profile(),
            Reply::Json(200, payment(frozen.snapshot())),
            Reply::Json(200, bad),
        ]);
        assert!(
            client(&server)
                .await
                .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
                .await
                .is_err()
        );
        assert_eq!(server.count(), 3);
    }
}
#[tokio::test]
async fn duplicate_original_bytes_and_repeated_offsets_fail() {
    let (_, frozen) = captured(38245);
    let good = page(
        ReversalKind::Refund,
        vec![item(ReversalKind::Refund)],
        Value::Null,
    )
    .to_string();
    for (before, after) in [
        ("\"count\":1", "\"count\":0,\"count\":1"),
        ("\"mode\":\"test\"", "\"mode\":\"live\",\"mode\":\"test\""),
        (
            "\"paymentId\":",
            "\"paymentId\":\"tr_OTHER\",\"paymentId\":",
        ),
    ] {
        assert!(good.contains(before));
        let server = Server::start(vec![
            profile(),
            Reply::Json(200, payment(frozen.snapshot())),
            Reply::Raw {
                status: 200,
                body: good.replace(before, after),
                mime: "application/json",
                chunked: false,
            },
        ]);
        assert!(
            client(&server)
                .await
                .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
                .await
                .is_err()
        );
    }
    let href = format!("https://api.mollie.com/v2/payments/{PAYMENT}/refunds?from=re_next");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, page(ReversalKind::Refund, vec![], link(&href))),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, page(ReversalKind::Refund, vec![], link(&href))),
    ]);
    let client = client(&server).await;
    let first = client
        .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("first");
    assert!(
        client
            .reversal_page(
                frozen.snapshot(),
                PAYMENT,
                ReversalKind::Refund,
                first.next_cursor()
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn chargeback_mode_and_original_financial_bindings_are_checked() {
    let (_, frozen) = captured(38245);
    for mode in [json!("live"), json!("invalid"), Value::Null] {
        let mut bad = item(ReversalKind::Chargeback);
        bad["mode"] = mode;
        let server = Server::start(vec![
            profile(),
            Reply::Json(200, payment(frozen.snapshot())),
            Reply::Json(200, page(ReversalKind::Chargeback, vec![bad], Value::Null)),
        ]);
        assert!(
            client(&server)
                .await
                .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Chargeback, None)
                .await
                .is_err()
        );
    }
    let mut explicit_test = item(ReversalKind::Chargeback);
    explicit_test["mode"] = json!("test");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(
            200,
            page(ReversalKind::Chargeback, vec![explicit_test], Value::Null),
        ),
    ]);
    assert_eq!(
        client(&server)
            .await
            .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Chargeback, None)
            .await
            .expect("test mode")
            .candidates()
            .len(),
        1
    );
    for pointer in [
        "/amount/value",
        "/metadata/order_id",
        "/metadata/attempt_id",
    ] {
        let mut parent = payment(frozen.snapshot());
        *parent.pointer_mut(pointer).expect("field") = if pointer == "/amount/value" {
            json!("382.44")
        } else {
            json!("f".repeat(32))
        };
        let server = Server::start(vec![profile(), Reply::Json(200, parent)]);
        assert!(
            client(&server)
                .await
                .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
                .await
                .is_err()
        );
        assert_eq!(server.count(), 2);
    }
}
#[tokio::test]
async fn reserved_characters_in_payment_and_cursor_are_scoped_and_encoded() {
    let (_, frozen) = captured(38245);
    let payment_id = "tr_opaque/with?query#fragment";
    let offset = "re_next/with?query#fragment";
    let mut parent = payment(frozen.snapshot());
    parent["id"] = json!(payment_id);
    let mut child = item(ReversalKind::Refund);
    child["paymentId"] = json!(payment_id);
    let mut next = url::Url::parse("https://api.mollie.com/v2/").expect("URL");
    next.path_segments_mut()
        .expect("hierarchical")
        .pop_if_empty()
        .extend(["payments", payment_id, "refunds"]);
    next.query_pairs_mut()
        .append_pair("from", offset)
        .append_pair("limit", "10");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, parent.clone()),
        Reply::Json(
            200,
            page(ReversalKind::Refund, vec![child], link(next.as_str())),
        ),
        Reply::Json(200, parent),
        Reply::Json(200, page(ReversalKind::Refund, vec![], Value::Null)),
    ]);
    let client = client(&server).await;
    let first = client
        .reversal_page(frozen.snapshot(), payment_id, ReversalKind::Refund, None)
        .await
        .expect("opaque payment");
    client
        .reversal_page(
            frozen.snapshot(),
            payment_id,
            ReversalKind::Refund,
            first.next_cursor(),
        )
        .await
        .expect("opaque cursor");
    server.inspect(|r| {
        assert_eq!(r[1].path, "/v2/payments/tr_opaque%2Fwith%3Fquery%23fragment");
        assert_eq!(r[4].path, "/v2/payments/tr_opaque%2Fwith%3Fquery%23fragment/refunds?limit=10&from=re_next%2Fwith%3Fquery%23fragment");
    });
}
