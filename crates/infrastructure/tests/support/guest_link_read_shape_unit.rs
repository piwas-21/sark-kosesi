use super::*;

fn healthy() -> LinkRow {
    LinkRow {
        order_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        linked_at: 1100,
        basket_id: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
        proof_created_at: Some(1000),
        proof_expires_at: Some(1900),
        reciprocal: true,
    }
}

#[test]
fn stored_link_shape_rejects_each_invalid_field_and_accepts_boundaries() {
    let row = healthy();
    assert_eq!(
        row.validate(900, 1100).expect("healthy link"),
        ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", 1000)
    );
    let mut cases: Vec<LinkRow> = Vec::new();
    let mut row = healthy();
    row.reciprocal = false;
    cases.push(row);
    let mut row = healthy();
    row.order_id = "wrong".into();
    cases.push(row);
    let mut row = healthy();
    row.basket_id = None;
    cases.push(row);
    let mut row = healthy();
    row.basket_id = Some("wrong".into());
    cases.push(row);
    let mut row = healthy();
    row.proof_created_at = None;
    cases.push(row);
    let mut row = healthy();
    row.proof_expires_at = None;
    cases.push(row);
    let mut row = healthy();
    row.linked_at = -1;
    cases.push(row);
    let mut row = healthy();
    row.proof_created_at = Some(-1);
    cases.push(row);
    let mut row = healthy();
    row.proof_expires_at = Some(MAX_QUOTE_UNIX_SECONDS + 1);
    cases.push(row);
    let mut row = healthy();
    row.proof_expires_at = Some(1901);
    cases.push(row);
    let mut row = healthy();
    row.proof_created_at = Some(1200);
    row.proof_expires_at = Some(2100);
    cases.push(row);
    let mut row = healthy();
    row.linked_at = 1900;
    cases.push(row);
    let mut row = healthy();
    row.linked_at = 1101;
    cases.push(row);
    for row in cases {
        assert!(matches!(
            row.validate(900, 1100),
            Err(LinkError::Unavailable)
        ));
    }
    assert!(matches!(
        healthy().validate(1101, 1100),
        Err(LinkError::Unavailable)
    ));
    let mut boundary = healthy();
    boundary.linked_at = 1000;
    assert!(boundary.validate(1000, 1000).is_ok());
    boundary.linked_at = 1899;
    assert!(boundary.validate(1899, 1899).is_ok());
}

#[test]
fn stored_account_identity_requires_canonical_mailbox_and_bounded_time() {
    assert!(valid_account("buyer@example.test", 0));
    assert!(valid_account("Buyer@example.test", MAX_QUOTE_UNIX_SECONDS));
    assert!(!valid_account("buyer@example.test", -1));
    assert!(!valid_account(
        "buyer@example.test",
        MAX_QUOTE_UNIX_SECONDS + 1
    ));
    assert!(!valid_account("buyer@EXAMPLE.test", 1100));
    assert!(!valid_account("not-an-email", 1100));
}

#[test]
fn historical_rows_require_exactly_one_result_per_requested_product() {
    assert!(titles::ensure_count(2, 2).is_ok());
    assert!(matches!(
        titles::ensure_count(1, 2),
        Err(LinkError::Unavailable)
    ));
    assert!(matches!(
        titles::ensure_count(3, 2),
        Err(LinkError::Unavailable)
    ));
    assert!(titles::ensure_count(0, 0).is_ok());
}
