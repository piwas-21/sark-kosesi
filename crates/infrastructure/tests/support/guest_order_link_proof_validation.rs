use super::*;

fn intact() -> ProofRow {
    ProofRow {
        order_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        basket_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        created_at: 100,
        expires_at: 1000,
        consumed_at: None,
        consumed_account_id: None,
        unconsumed_xid: true,
    }
}

#[test]
fn damaged_proof_headers_never_authorize_redemption() {
    let order = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert!(intact().active(order, 100));
    assert!(intact().active(order, 999));
    assert!(!intact().active(order, 99));
    assert!(!intact().active(order, 1000));
    assert!(!intact().active("cccccccccccccccccccccccccccccccc", 100));

    let mut row = intact();
    row.basket_id = "not-hex".into();
    assert!(!row.active(order, 100));
    let mut row = intact();
    row.created_at = -1;
    assert!(!row.active(order, 100));
    let mut row = intact();
    row.expires_at = 1001;
    assert!(!row.active(order, 100));
    let mut row = intact();
    row.created_at = i64::MAX;
    row.expires_at = i64::MAX;
    assert!(!row.active(order, i64::MAX));
    let mut row = intact();
    row.consumed_at = Some(100);
    assert!(!row.active(order, 100));
    let mut row = intact();
    row.consumed_account_id = Some(1);
    assert!(!row.active(order, 100));
    let mut row = intact();
    row.unconsumed_xid = false;
    assert!(!row.active(order, 100));
}
