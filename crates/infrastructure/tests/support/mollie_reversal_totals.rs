//! Authenticated loopback GET only; no provider or customer data.
use super::*;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";

async fn get(
    reply: Reply,
    attempt: &AttemptSnapshot,
) -> Result<VerifiedReversalTotals, MollieError> {
    let server = Server::start(vec![profile(), reply]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified test profile")
        .into_reconciliation();
    let result = client.fetch_reversal_totals(attempt, PAYMENT).await;
    server.inspect(|requests| {
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/v2/profiles/me");
        assert_eq!(requests[1].path, format!("/v2/payments/{PAYMENT}"));
        for request in requests {
            assert_eq!(request.method, "GET");
            assert!(request.body.is_empty());
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {}", raw_key()).as_str())
            );
        }
    });
    result
}

#[tokio::test]
async fn preserves_optional_provider_totals_without_inferring_financial_state() {
    let (_, frozen) = captured(38245);
    let source = payment(frozen.snapshot());
    let absent = get(Reply::Json(200, source.clone()), frozen.snapshot())
        .await
        .expect("paid original with no aggregate fields");
    assert_eq!(
        absent.totals(),
        ReversalTotals {
            refunded_minor: None,
            remaining_minor: None,
            captured_minor: None,
            charged_back_minor: None,
        }
    );
    let mut full = source;
    full["amountRefunded"] = json!({"currency":"EUR","value":"395.01"});
    full["amountRemaining"] = json!({"currency":"EUR","value":"0.00"});
    full["amountCaptured"] = json!({"currency":"EUR","value":"382.45"});
    full["amountChargedBack"] = json!({"currency":"EUR","value":"10.05"});
    let bound = get(Reply::Json(200, full), frozen.snapshot())
        .await
        .expect("original with all optional aggregates");
    assert_eq!(bound.attempt().binding.id, "1".repeat(32));
    assert_eq!(bound.attempt().binding.order_id, "2".repeat(32));
    assert_eq!(bound.attempt().binding.amount_minor, 38245);
    assert_eq!(bound.payment_id(), PAYMENT);
    assert_eq!(
        bound.credential_fingerprint(),
        credential().binding().credential_fingerprint
    );
    assert_eq!(
        bound.totals(),
        ReversalTotals {
            refunded_minor: Some(39501),
            remaining_minor: Some(0),
            captured_minor: Some(38245),
            charged_back_minor: Some(1005),
        }
    );
}

#[tokio::test]
async fn malformed_present_totals_and_mismatched_originals_fail_closed() {
    let (_, frozen) = captured(38245);
    let source = payment(frozen.snapshot());
    for invalid in [
        Value::Null,
        json!("EUR 0.00"),
        json!({"currency":"USD","value":"0.00"}),
        json!({"currency":"EUR","value":"-0.01"}),
        json!({"currency":"EUR","value":"0.001"}),
        json!({"currency":"EUR","value":"90071992547409.92"}),
        json!({"currency":"EUR","value":12.34}),
        json!({"currency":"EUR","value":"0.01","valueMinor":1}),
    ] {
        let mut item = source.clone();
        item["amountRefunded"] = invalid;
        assert!(matches!(
            get(Reply::Json(200, item), frozen.snapshot()).await,
            Err(MollieError::InvalidResponse)
        ));
    }
    for (field, value) in [
        ("status", json!("pending")),
        ("profileId", json!("pfl_WRONG")),
        ("id", json!("tr_OTHER")),
        ("amount", json!({"currency":"EUR","value":"382.44"})),
        (
            "metadata",
            json!({"schema_version":1,"order_id":"a".repeat(32),"attempt_id":"1".repeat(32)}),
        ),
    ] {
        let mut item = source.clone();
        item[field] = value;
        assert!(
            matches!(
                get(Reply::Json(200, item), frozen.snapshot()).await,
                Err(MollieError::PaymentMismatch)
            ),
            "{field}"
        );
    }
    let mut live = source.clone();
    live["mode"] = "live".into();
    assert!(matches!(
        get(Reply::Json(200, live), frozen.snapshot()).await,
        Err(MollieError::InvalidResponse)
    ));
    for prefix in [
        r#""amountRemaining":{"currency":"EUR","value":"12.34"},"amountRemaining":{"currency":"EUR","value":"0.00"},"#,
        r#""amountRemaining":{"currency":"EUR","value":"12.34","value":"0.00"},"#,
    ] {
        let body = format!("{{{prefix}{}", &source.to_string()[1..]);
        let reply = Reply::Raw {
            status: 200,
            body,
            mime: "application/json",
            chunked: false,
        };
        assert!(matches!(
            get(reply, frozen.snapshot()).await,
            Err(MollieError::InvalidResponse)
        ));
    }
    assert!(
        get(Reply::Json(404, json!({})), frozen.snapshot())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn malformed_locator_and_other_profile_do_not_send_a_payment_request() {
    let (_, frozen) = captured(38245);
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified test profile")
        .into_reconciliation();
    assert!(matches!(
        client.fetch_reversal_totals(frozen.snapshot(), "tr_").await,
        Err(MollieError::PaymentMismatch)
    ));
    let key = TestCredential::capture(&raw_key(), "pfl_OTHER").expect("other test profile");
    let configuration = config(&key);
    let mut other = frozen.snapshot().clone();
    other.binding.profile = key.binding().clone();
    other.binding.configuration_sha256 = configuration.identity().sha256;
    assert!(other.validate().is_ok());
    assert!(matches!(
        client.fetch_reversal_totals(&other, PAYMENT).await,
        Err(MollieError::PaymentMismatch)
    ));
    server.inspect(|requests| assert_eq!(requests.len(), 1));
}
