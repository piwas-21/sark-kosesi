use super::*;

#[sqlx::test(migrations = false)]
async fn schema39_preserves_existing_verified_identities_and_pending_proofs(pool: PgPool) {
    MIGRATOR.run_to(38, &pool).await.expect("schema 38");
    let time = clock();
    let established = "existing-customer@example.com";
    let pending = "pending-customer@example.com";
    let established_id = account(&pool, established, &time).await;
    let pending_proof = email_verification::issue(&pool, pending, &time)
        .await
        .expect("pending mailbox proof before upgrade");
    let before: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM verified_customer_accounts a ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("verified identities before upgrade");

    MIGRATOR.run_to(39, &pool).await.expect("schema 39");
    let after: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM verified_customer_accounts a ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("verified identities after upgrade");
    assert_eq!(after, before);
    assert_eq!(counts(&pool).await, (0, 0));

    assert!(matches!(
        customer_passwords::issue(&pool, pending, &time).await,
        Err(PasswordError::Denied)
    ));
    let change = customer_passwords::issue(&pool, established, &time)
        .await
        .expect("password proof for already verified account");
    assert_eq!(
        customer_passwords::replace(&pool, established, &change, "new fixture password", &time)
            .await,
        Ok(1)
    );
    let credential_owner: i64 =
        sqlx::query_scalar("SELECT account_id FROM customer_password_credentials")
            .fetch_one(&pool)
            .await
            .expect("first password belongs to the established account");
    assert_eq!(credential_owner, established_id);

    let verified_after_upgrade = establish(&pool, pending, &pending_proof, &time)
        .await
        .expect("pre-upgrade mailbox proof retains its purpose");
    assert_ne!(verified_after_upgrade.value(), established_id);
    assert_eq!(counts(&pool).await, (1, 1));
}
