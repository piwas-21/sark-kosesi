//! Authenticated loopback tests only; synthetic payment and reversal data.
use super::*;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";
const REFUND: &str = "re_SYNTHETIC";
const CHARGEBACK: &str = "chb_SYNTHETIC";
fn refund() -> Value {
    json!({"resource":"refund","id":REFUND,"paymentId":PAYMENT,"mode":"test",
        "amount":{"currency":"EUR","value":"12.34"},"status":"pending",
        "createdAt":"2024-03-20T09:13:37+00:00","description":"private marker","metadata":{"private":"marker"}})
}
fn chargeback() -> Value {
    json!({"resource":"chargeback","id":CHARGEBACK,"paymentId":PAYMENT,
        "amount":{"currency":"EUR","value":"12.34"},"createdAt":"2024-03-20T09:13:37+00:00",
        "reversedAt":null,"reason":{"description":"private marker"}})
}
async fn run(child: Reply, is_chargeback: bool) -> Result<VerifiedReversal, MollieError> {
    let (_, f) = captured(38245);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(f.snapshot())),
        child,
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let result = if is_chargeback {
        client
            .fetch_chargeback(f.snapshot(), PAYMENT, CHARGEBACK)
            .await
    } else {
        client.fetch_refund(f.snapshot(), PAYMENT, REFUND).await
    };
    server.inspect(|r| {
        assert_eq!(r.len(), 3);
        for request in r {
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {}", raw_key()).as_str())
            );
            assert!(request.body.is_empty());
        }
        assert_eq!(r[1].path, format!("/v2/payments/{PAYMENT}"));
        let (collection, id) = if is_chargeback {
            ("chargebacks", CHARGEBACK)
        } else {
            ("refunds", REFUND)
        };
        assert_eq!(
            r[2].path,
            format!("/v2/payments/{PAYMENT}/{collection}/{id}")
        );
    });
    result
}
#[tokio::test]
async fn exact_observation_retains_minimal_bound_provenance_and_all_documented_phases() {
    for (status, phase) in [
        ("queued", ReversalPhase::RefundQueued),
        ("pending", ReversalPhase::RefundPending),
        ("processing", ReversalPhase::RefundProcessing),
        ("refunded", ReversalPhase::RefundRefunded),
        ("failed", ReversalPhase::RefundFailed),
        ("canceled", ReversalPhase::RefundCanceled),
    ] {
        let mut item = refund();
        item["status"] = status.into();
        let result = run(Reply::Json(200, item), false).await.expect("refund");
        assert_eq!(result.phase(), phase);
        assert_eq!(result.attempt().binding.id, "1".repeat(32));
        assert_eq!(result.attempt().binding.order_id, "2".repeat(32));
        assert_eq!(result.payment_id(), PAYMENT);
        assert_eq!(result.child_id(), REFUND);
        assert_eq!(result.amount_minor(), 1234);
        assert_eq!(
            result.credential_fingerprint(),
            credential().binding().credential_fingerprint
        );
        assert_eq!(result.created_at().unix_timestamp(), 1710926017);
        assert!(result.reversed_at().is_none());
    }
    let result = run(Reply::Json(200, chargeback()), true)
        .await
        .expect("chargeback");
    assert_eq!(result.phase(), ReversalPhase::ChargebackReceived);
    let mut item = chargeback();
    item["reversedAt"] = "2024-03-21T09:13:37+00:00".into();
    let result = run(Reply::Json(200, item), true).await.expect("reversed");
    assert_eq!(result.phase(), ReversalPhase::ChargebackReversed);
    assert_eq!(
        result.reversed_at().expect("timestamp").unix_timestamp(),
        1711012417
    );
}
#[tokio::test]
async fn child_identity_money_mode_phase_and_timestamps_fail_closed() {
    for (pointer, value) in [
        ("/resource", json!("chargeback")),
        ("/id", json!("re_OTHER")),
        ("/paymentId", json!("tr_OTHER")),
        ("/mode", json!("live")),
        ("/amount/currency", json!("USD")),
        ("/amount/value", json!("0.00")),
        ("/amount/value", json!("382.46")),
        ("/amount/value", json!("01.00")),
        ("/amount/value", json!(12.34)),
        ("/amount", json!(["EUR", "12.34"])),
        ("/status", json!("unknown")),
        ("/createdAt", json!("2024-02-30T09:13:37Z")),
    ] {
        let mut item = refund();
        *item.pointer_mut(pointer).expect("field") = value;
        assert!(
            run(Reply::Json(200, item), false).await.is_err(),
            "{pointer}"
        );
    }
    for (pointer, value) in [
        ("/resource", json!("refund")),
        ("/id", json!("chb_OTHER")),
        ("/paymentId", json!("tr_OTHER")),
        ("/amount/value", json!("382.46")),
        ("/reversedAt", json!(false)),
        ("/reversedAt", json!("2024-02-30T00:00:00Z")),
        ("/reversedAt", json!("not a time")),
        ("/reversedAt", json!("2024-03-19T09:13:37+00:00")),
        ("/createdAt", json!("")),
    ] {
        let mut item = chargeback();
        *item.pointer_mut(pointer).expect("field") = value;
        assert!(
            run(Reply::Json(200, item), true).await.is_err(),
            "{pointer}"
        );
    }
    for mode in [json!("live"), Value::Null] {
        let mut item = chargeback();
        item["mode"] = mode;
        assert!(run(Reply::Json(200, item), true).await.is_err());
    }
    assert!(matches!(
        run(
            Reply::Json(
                200,
                json!(["refund", REFUND, PAYMENT, "test", {"currency":"EUR","value":"12.34"}, "pending", "2024-03-20T09:13:37Z"]),
            ),
            false,
        )
        .await,
        Err(MollieError::InvalidResponse)
    ));
    let mut item = chargeback();
    item.as_object_mut().expect("object").remove("reversedAt");
    assert!(run(Reply::Json(200, item), true).await.is_err());
    assert!(matches!(
        run(Reply::Json(404, json!({})), false).await,
        Err(MollieError::Unavailable)
    ));
    assert!(matches!(
        run(Reply::Json(503, json!({})), true).await,
        Err(MollieError::Unavailable)
    ));
}
#[tokio::test]
async fn duplicate_critical_fields_are_rejected_without_lossy_projection() {
    for (source, old, replacement, chargeback) in [
        (
            refund(),
            "\"status\":\"pending\"",
            "\"status\":\"failed\",\"status\":\"pending\"",
            false,
        ),
        (
            refund(),
            "\"value\":\"12.34\"",
            "\"value\":\"1.00\",\"value\":\"12.34\"",
            false,
        ),
        (
            refund(),
            "\"mode\":\"test\"",
            "\"mode\":\"live\",\"mode\":\"test\"",
            false,
        ),
        (
            chargeback(),
            "\"reversedAt\":null",
            "\"reversedAt\":\"bad\",\"reversedAt\":null",
            true,
        ),
    ] {
        let raw = source.to_string();
        assert!(raw.contains(old));
        let reply = Reply::Raw {
            status: 200,
            body: raw.replace(old, replacement),
            mime: "application/json",
            chunked: false,
        };
        assert!(matches!(
            run(reply, chargeback).await,
            Err(MollieError::InvalidResponse)
        ));
    }
}
#[tokio::test]
async fn invalid_ids_and_original_payment_never_reach_child_get() {
    let (_, f) = captured(38245);
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    for id in [
        "",
        "re_",
        "chb_bad",
        "re_ bad\n",
        &format!("re_{}", "x".repeat(254)),
    ] {
        assert!(
            client
                .fetch_refund(f.snapshot(), PAYMENT, id)
                .await
                .is_err()
        );
    }
    for id in [
        "",
        "chb_",
        "re_bad",
        "chb_bad\n",
        &format!("chb_{}", "x".repeat(253)),
    ] {
        assert!(
            client
                .fetch_chargeback(f.snapshot(), PAYMENT, id)
                .await
                .is_err()
        );
    }
    assert!(
        client
            .fetch_refund(f.snapshot(), "tr_", REFUND)
            .await
            .is_err()
    );
    let mut wrong = f.snapshot().clone();
    wrong.binding.profile.id = "pfl_OTHER".into();
    assert!(client.fetch_refund(&wrong, PAYMENT, REFUND).await.is_err());
    assert_eq!(server.count(), 1);
    for (pointer, value) in [
        ("/status", json!("pending")),
        ("/mode", json!("live")),
        ("/profileId", json!("pfl_OTHER")),
        ("/amount/value", json!("382.44")),
        ("/metadata/order_id", json!("f".repeat(32))),
        ("/metadata/attempt_id", json!("f".repeat(32))),
    ] {
        let mut response = payment(f.snapshot());
        *response.pointer_mut(pointer).expect("field") = value;
        let server = Server::start(vec![profile(), Reply::Json(200, response)]);
        let client = local(&server)
            .verify_profile()
            .await
            .expect("profile")
            .into_reconciliation();
        assert!(
            client
                .fetch_refund(f.snapshot(), PAYMENT, REFUND)
                .await
                .is_err()
        );
        assert_eq!(server.count(), 2, "{pointer}");
    }
}

#[tokio::test]
async fn child_opaque_segments_are_encoded_and_missing_parent_cannot_prove_absence() {
    let (_, f) = captured(38245);
    let mut item = refund();
    item["id"] = "re_part/path?query#fragment".into();
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(f.snapshot())),
        Reply::Json(200, item),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    assert!(
        client
            .fetch_refund(f.snapshot(), PAYMENT, "re_part/path?query#fragment")
            .await
            .is_ok()
    );
    server.inspect(|r| {
        assert_eq!(
            r[2].path,
            format!("/v2/payments/{PAYMENT}/refunds/re_part%2Fpath%3Fquery%23fragment")
        )
    });
    let server = Server::start(vec![profile(), Reply::Json(404, json!({}))]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    assert!(matches!(
        client.fetch_refund(f.snapshot(), PAYMENT, REFUND).await,
        Err(MollieError::Unavailable)
    ));
    assert_eq!(server.count(), 2);
}
