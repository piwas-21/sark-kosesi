use super::*;

/// Issue a proof only while the guest owns the exact validated order and CSRF claim.
pub async fn issue(
    store: &GuestSessionStore,
    guest: &GuestCredential,
    csrf: &str,
    order_id: &str,
) -> Result<GuestOrderLinkToken, LinkError> {
    if !valid_hex(order_id, 32) {
        return Err(LinkError::Denied);
    }
    let mut scope = access::lock(store, guest, Some(csrf)).await?;
    let basket_id = scope.identity.basket_id.clone();
    require_unlinked_order(&mut scope.tx, &basket_id, order_id).await?;
    let snapshot =
        checkout_orders::guest_snapshot_in_transaction(&mut scope.tx, &basket_id, order_id)
            .await
            .map_err(order_error)?;
    commerce_domain::email_identity::normalize_email(&snapshot.buyer.email)
        .ok_or(LinkError::Denied)?;
    let created_at = scope.checked_at(store.clock.as_ref())?;
    let expires_at = created_at
        .checked_add(LIFETIME)
        .ok_or(LinkError::Unavailable)?;
    let token = GuestOrderLinkToken::random()?;
    let verifier = token.verifier();
    let inserted = sqlx::query("INSERT INTO guest_order_link_proofs(verifier,order_id,basket_id,created_at,expires_at,consumed_at,consumed_account_id,consumed_xid) VALUES ($1,$2,$3,$4,$5,NULL,NULL,NULL)")
        .bind(verifier.as_slice()).bind(order_id).bind(&basket_id).bind(created_at).bind(expires_at)
        .execute(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(LinkError::Unavailable);
    }
    let stored: Option<(bool,)> = sqlx::query_as("SELECT order_id=$2 AND basket_id=$3 AND created_at=$4 AND expires_at=$5 AND consumed_at IS NULL AND consumed_account_id IS NULL AND consumed_xid IS NULL FROM guest_order_link_proofs WHERE verifier=$1")
        .bind(verifier.as_slice()).bind(order_id).bind(&basket_id).bind(created_at).bind(expires_at)
        .fetch_optional(&mut *scope.tx).await.map_err(|_| LinkError::Unavailable)?;
    if stored != Some((true,)) {
        return Err(LinkError::Unavailable);
    }
    let final_at = scope.checked_at(store.clock.as_ref())?;
    // The scope rejects a clock that moves backward before returning final_at.
    if final_at >= expires_at {
        return Err(LinkError::Denied);
    }
    scope
        .tx
        .commit()
        .await
        .map_err(|_| LinkError::Unavailable)?;
    Ok(token)
}
