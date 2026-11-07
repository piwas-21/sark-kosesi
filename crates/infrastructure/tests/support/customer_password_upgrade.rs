use super::*;

#[sqlx::test(migrations = false)]
async fn schema39_preserves_existing_verified_identities_and_pending_proofs(pool: PgPool) {
    MIGRATOR.run_to(38, &pool).await.expect("schema 38");
    let time = clock();
    let established = "existing-customer@example.com";
    let pending = "pending-customer@example.com";
    // At schema 38, the current admitted entry points cannot run until 0043.
    // Build the same native proof/account custody directly in this isolated DB.
    let pending_proof =
        commerce_infrastructure::email_verification::VerificationToken::from_bytes([42; 32]);
    let pending_verifier = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(pending_proof.expose_bytes())
        .finalize()
        .to_vec();
    let established_verifier = vec![19_u8; 32];
    let mut tx = pool.begin().await.expect("fixture transaction");
    for (email, verifier) in [
        (established, &established_verifier),
        (pending, &pending_verifier),
    ] {
        sqlx::query("INSERT INTO email_verification_challenges (verifier,purpose,email_identity,created_at,expires_at) VALUES ($1,'email-verification-v1',$2,1000,1900)")
            .bind(verifier).bind(email).execute(&mut *tx).await.expect("schema 38 proof");
    }
    sqlx::query("UPDATE email_verification_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(&established_verifier)
        .execute(&mut *tx)
        .await
        .expect("consume exact proof");
    let established_id: i64 = sqlx::query_scalar("INSERT INTO verified_customer_accounts (email_identity,verification_verifier,verified_at) VALUES ($1,$2,1000) RETURNING id")
        .bind(established).bind(&established_verifier).fetch_one(&mut *tx).await.expect("schema 38 account");
    tx.commit().await.expect("commit fixture custody");
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
    let pending_intact: bool = sqlx::query_scalar("SELECT consumed_at IS NULL AND consumed_xid IS NULL FROM email_verification_challenges WHERE verifier=$1 AND email_identity=$2")
        .bind(&pending_verifier).bind(pending).fetch_one(&pool).await.expect("pending proof after schema 39");
    assert!(pending_intact);

    // The current API requires durable admission; continue the real migration
    // chain only after observing preservation at exactly schema 39.
    MIGRATOR.run_to(43, &pool).await.expect("schema 43");

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
