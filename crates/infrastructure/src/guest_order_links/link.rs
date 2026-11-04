use super::*;
use commerce_domain::email_identity::normalize_email;

#[derive(sqlx::FromRow)]
struct ProofRow {
    order_id: String,
    basket_id: String,
    created_at: i64,
    expires_at: i64,
    consumed_at: Option<i64>,
    consumed_account_id: Option<i64>,
    unconsumed_xid: bool,
}
impl ProofRow {
    fn active(&self, order_id: &str, at: i64) -> bool {
        self.order_id == order_id
            && valid_hex(&self.basket_id, 32)
            && self.created_at >= 0
            && self.created_at <= at
            && at < self.expires_at
            && self.created_at.checked_add(LIFETIME) == Some(self.expires_at)
            && self.consumed_at.is_none()
            && self.consumed_account_id.is_none()
            && self.unconsumed_xid
    }
}
fn time(clock: &dyn Clock) -> Result<i64, LinkError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(LinkError::Unavailable)
}

/// Consume one proof and link its exact validated order to the authenticated account.
pub async fn link(
    pool: &PgPool,
    customer: &CustomerSessionToken,
    order_id: &str,
    proof: &GuestOrderLinkToken,
    clock: &dyn Clock,
) -> Result<LinkedOrder, LinkError> {
    if !valid_hex(order_id, 32) {
        return Err(LinkError::Denied);
    }
    let observed = ObservedClock::new(clock, 0);
    time(&observed)?;
    let mut tx = customer_sessions::begin(pool).await?;
    let session = customer_sessions::claim_session(&mut tx, customer, &observed).await?;
    let account_id = session.account_id();
    let verifier = proof.verifier();
    let row: Option<ProofRow> = sqlx::query_as("SELECT left(order_id,33) AS order_id,left(basket_id,33) AS basket_id,created_at,expires_at,consumed_at,consumed_account_id,consumed_xid IS NULL AS unconsumed_xid FROM guest_order_link_proofs WHERE verifier=$1 FOR UPDATE")
        .bind(verifier.as_slice()).fetch_optional(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    let row = row.ok_or(LinkError::Denied)?;
    if !row.active(order_id, time(&observed)?) {
        return Err(LinkError::Denied);
    }
    // The proof lock precedes this order lock for all link attempts.
    require_unlinked_order(&mut tx, &row.basket_id, order_id).await?;
    let snapshot = checkout_orders::snapshot_in_transaction(&mut tx, &row.basket_id, order_id)
        .await
        .map_err(order_error)?;
    let email: Option<String> = sqlx::query_scalar(
        "SELECT left(email_identity,255) FROM verified_customer_accounts WHERE id=$1",
    )
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    let email = email.ok_or(LinkError::Denied)?;
    let buyer = normalize_email(&snapshot.buyer.email).ok_or(LinkError::Denied)?;
    if normalize_email(&email).as_deref() != Some(email.as_str()) || buyer != email {
        return Err(LinkError::Denied);
    }
    let linked_at = time(&observed)?;
    session.check_at(linked_at)?;
    if !row.active(order_id, linked_at) {
        return Err(LinkError::Denied);
    }
    let updated = sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=$2,consumed_account_id=$3,consumed_xid=NULL WHERE verifier=$1 AND consumed_at IS NULL AND consumed_account_id IS NULL AND consumed_xid IS NULL")
        .bind(verifier.as_slice()).bind(linked_at).bind(account_id).execute(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(LinkError::Unavailable);
    }
    let inserted = sqlx::query("INSERT INTO customer_order_links(order_id,account_id,proof_verifier,linked_at,linked_xid) VALUES ($1,$2,$3,$4,NULL)")
        .bind(order_id).bind(account_id).bind(verifier.as_slice()).bind(linked_at).execute(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(LinkError::Unavailable);
    }
    sqlx::query("SET CONSTRAINTS require_guest_order_link IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| LinkError::Unavailable)?;
    let stored: Option<(bool,)> = sqlx::query_as("SELECT p.order_id=$2 AND p.basket_id=$3 AND p.created_at=$4 AND p.expires_at=$5 AND p.consumed_at=$6 AND p.consumed_account_id=$7 AND p.consumed_xid=pg_current_xact_id() AND l.account_id=$7 AND l.proof_verifier=$1 AND l.linked_at=$6 AND l.linked_xid=pg_current_xact_id() FROM guest_order_link_proofs p JOIN customer_order_links l ON l.order_id=p.order_id WHERE p.verifier=$1")
        .bind(verifier.as_slice()).bind(order_id).bind(&row.basket_id).bind(row.created_at).bind(row.expires_at).bind(linked_at).bind(account_id)
        .fetch_optional(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    if stored != Some((true,)) {
        return Err(LinkError::Unavailable);
    }
    let final_at = time(&observed)?;
    session.check_at(final_at)?;
    if !row.active(order_id, final_at) {
        return Err(LinkError::Denied);
    }
    tx.commit().await.map_err(|_| LinkError::Unavailable)?;
    Ok(LinkedOrder {
        order_id: order_id.to_owned(),
        account_id,
        linked_at,
    })
}

#[cfg(test)]
#[path = "../../tests/support/guest_order_link_proof_validation.rs"]
mod tests;
