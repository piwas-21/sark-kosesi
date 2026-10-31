use super::*;
use commerce_domain::checkout_quotes::{CapturedAmount, CapturedLine};
fn snapshot() -> QuoteSnapshot {
    QuoteSnapshot {
        schema_version: 1,
        binding: QuoteBinding {
            quote_id: "1".repeat(32),
            basket_id: "2".repeat(32),
            basket_revision: 1,
            locale: "nl".into(),
            country: "NL".into(),
            postcode: "1234 AB".into(),
            policy_id: "POLICY-NL".into(),
            policy_revision: 1,
            policy_review_revision: 2,
            created_at: 1100,
            expires_at: 1400,
        },
        currency: "EUR".into(),
        lines: vec![CapturedLine {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
            bom_revision: 1,
            quantity: 3,
            unit_gross_minor: 100,
            vat_basis_points: 2100,
            amounts: CapturedAmount {
                gross_minor: 300,
                net_minor: 248,
                vat_minor: 52,
            },
        }],
        shipping: CapturedAmount {
            gross_minor: 900,
            net_minor: 744,
            vat_minor: 156,
        },
        shipping_vat_basis_points: 2100,
        total: CapturedAmount {
            gross_minor: 1200,
            net_minor: 992,
            vat_minor: 208,
        },
    }
}
fn row() -> Row {
    let s = snapshot();
    let b = &s.binding;
    Row {
        id: b.quote_id.clone(),
        basket_id: b.basket_id.clone(),
        basket_revision: b.basket_revision,
        effect_key: "first".into(),
        locale: b.locale.clone(),
        country: b.country.clone(),
        postcode: b.postcode.clone(),
        policy_id: b.policy_id.clone(),
        policy_revision: b.policy_revision,
        policy_review_revision: b.policy_review_revision,
        created_at: b.created_at,
        expires_at: b.expires_at,
        snapshot: Some(serde_json::to_value(s).expect("fixture")),
    }
}
#[test]
fn stored_snapshot_requires_object_shapes_exact_headers_and_independent_money_checks() {
    let decoded = row().decode().expect("positive stored control");
    assert!(decoded.0 == snapshot());
    assert_eq!(decoded.1, "first");
    for corruption in 0..21 {
        let mut r = row();
        let v = r.snapshot.as_mut().expect("value");
        match corruption {
            0 => r.snapshot = None,
            1 => *v = serde_json::json!([]),
            2 => v["binding"] = serde_json::json!([]),
            3 => v["shipping"] = serde_json::json!([900, 744, 156]),
            4 => v["total"] = serde_json::json!([1200, 992, 208]),
            5 => v["lines"] = serde_json::json!({}),
            6 => v["lines"] = serde_json::json!([[]]),
            7 => v["lines"][0]["amounts"] = serde_json::json!([300, 248, 52]),
            8 => v["lines"] = serde_json::json!(vec![v["lines"][0].clone(); 21]),
            9 => v["unexpected"] = true.into(),
            10 => r.id = "3".repeat(32),
            11 => r.basket_id = "3".repeat(32),
            12 => r.basket_revision = 2,
            13 => r.locale = "de".into(),
            14 => r.postcode = "9999 XX".into(),
            15 => r.policy_review_revision = 3,
            16 => r.expires_at = 1401,
            17 => r.effect_key = " ".into(),
            18 => v["total"]["gross_minor"] = 1201.into(),
            19 => v["currency"] = "USD".into(),
            _ => v["lines"][0]["bom_revision"] = 0.into(),
        }
        assert!(r.decode().is_err(), "corruption {corruption}");
    }
}
