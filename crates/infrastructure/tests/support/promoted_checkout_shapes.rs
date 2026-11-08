use super::*;
use serde_json::{Value, json};

#[sqlx::test(migrator = "MIGRATOR")]
async fn restored_quote_rejects_nonobjects_and_inconsistent_promotion_headers(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let owner = GuestCredential::from_session(&session).expect("owner");
    let receipt = issue_promoted_quote(
        &store,
        &owner,
        &csrf,
        Locale::Nl,
        &fixture::request("shape"),
        "COURTYARD",
    )
    .await
    .expect("valid source");
    let id = &receipt.snapshot.original.binding.quote_id;
    let source = serde_json::to_value(&receipt.snapshot).expect("valid envelope");
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
        .await
        .expect("simulate damaged isolated restore");
    let array_amount = json!([38245, 31607, 6638]);
    let cases = [
        ("/original", Value::Null),
        ("/promotion", json!(["COURTYARD", 1, 1])),
        ("/shipping", array_amount.clone()),
        ("/total", array_amount.clone()),
        ("/lines", json!({})),
        ("/lines", json!(vec![source["lines"][0].clone(); 21])),
        ("/lines/0", json!(["SET-FILLED", 500, array_amount.clone()])),
        ("/lines/0/amounts", array_amount),
        ("/promotion/id", json!("OTHER")),
        ("/promotion/revision", json!(2)),
        ("/promotion/review_revision", json!(2)),
    ];
    for (pointer, replacement) in cases {
        let mut damaged = source.clone();
        *damaged.pointer_mut(pointer).expect("fixture field") = replacement;
        sqlx::query("UPDATE guest_quote_snapshots SET snapshot=$2 WHERE id=$1")
            .bind(id)
            .bind(damaged)
            .execute(&pool)
            .await
            .expect("isolated restore fault");
        assert!(
            matches!(
                owned_promoted_quote(&store, &owner, id).await,
                Err(QuoteStoreError::InvalidStoredData)
            ),
            "corrupt field {pointer}"
        );
    }
    sqlx::query("UPDATE guest_quote_snapshots SET snapshot=$2 WHERE id=$1")
        .bind(id)
        .bind(source)
        .execute(&pool)
        .await
        .expect("restore exact envelope");
    pool.execute("ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote")
        .await
        .expect("restore immutable guard");
    assert!(
        owned_promoted_quote(&store, &owner, id)
            .await
            .expect("intact control")
            == receipt.snapshot
    );
}
