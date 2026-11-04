use super::*;

#[cfg(test)]
#[path = "../../tests/support/guest_link_read_shape_unit.rs"]
mod tests;
mod titles;
use commerce_domain::{
    checkout_quotes::{CapturedAmount, MAX_QUOTE_UNIX_SECONDS},
    email_identity::normalize_email,
};

/// The bounded, immutable commercial details of a linked order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedOrderView {
    pub order_id: String,
    pub created_at: i64,
    pub linked_at: i64,
    pub currency: String,
    pub lines: Vec<LinkedOrderLine>,
    pub shipping: CapturedAmount,
    pub total: CapturedAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedOrderLine {
    pub sku: String,
    pub title: String,
    pub quantity: u32,
    pub amounts: CapturedAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedOrderCursor {
    pub linked_at: i64,
    pub order_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedOrderPage {
    pub orders: Vec<LinkedOrderView>,
    pub next_cursor: Option<LinkedOrderCursor>,
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    order_id: String,
    linked_at: i64,
    basket_id: Option<String>,
    proof_created_at: Option<i64>,
    proof_expires_at: Option<i64>,
    reciprocal: bool,
}

fn time(clock: &dyn Clock) -> Result<i64, LinkError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(LinkError::Unavailable)
}

fn valid_timestamp(value: i64) -> bool {
    (0..=MAX_QUOTE_UNIX_SECONDS).contains(&value)
}

fn valid_account(email: &str, verified_at: i64) -> bool {
    valid_timestamp(verified_at) && normalize_email(email).as_deref() == Some(email)
}

impl LinkRow {
    fn validate(&self, verified_at: i64, at: i64) -> Result<(&str, i64), LinkError> {
        let basket_id = self.basket_id.as_deref().ok_or(LinkError::Unavailable)?;
        let created = self.proof_created_at.ok_or(LinkError::Unavailable)?;
        let expires = self.proof_expires_at.ok_or(LinkError::Unavailable)?;
        if !self.reciprocal
            || !valid_hex(&self.order_id, 32)
            || !valid_hex(basket_id, 32)
            || !valid_timestamp(self.linked_at)
            || !valid_timestamp(created)
            || !valid_timestamp(expires)
            || created.checked_add(LIFETIME) != Some(expires)
            || created > self.linked_at
            || self.linked_at >= expires
            || self.linked_at > at
            || verified_at > self.linked_at
        {
            return Err(LinkError::Unavailable);
        }
        Ok((basket_id, created))
    }
}

async fn account_email(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(String, i64), LinkError> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT left(email_identity,255),verified_at FROM verified_customer_accounts WHERE id=$1",
    )
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    let (email, verified_at) = row.ok_or(LinkError::Unavailable)?;
    if !valid_account(&email, verified_at) {
        return Err(LinkError::Unavailable);
    }
    Ok((email, verified_at))
}

async fn project(
    tx: &mut Transaction<'_, Postgres>,
    row: LinkRow,
    email: &str,
    verified_at: i64,
    at: i64,
) -> Result<LinkedOrderView, LinkError> {
    let (basket_id, created) = row.validate(verified_at, at)?;
    // A missing order behind an owned link is corruption, not an absent customer order.
    let snapshot = checkout_orders::snapshot_in_transaction(tx, basket_id, &row.order_id)
        .await
        .map_err(|_| LinkError::Unavailable)?;
    if snapshot.identity.created_at > created
        || normalize_email(&snapshot.buyer.email).as_deref() != Some(email)
    {
        return Err(LinkError::Unavailable);
    }
    titles::validate_titles(tx, &snapshot).await?;
    let lines = snapshot
        .quote
        .lines
        .into_iter()
        .zip(snapshot.products)
        .map(|(line, product)| LinkedOrderLine {
            sku: line.sku,
            title: product.title,
            quantity: line.quantity,
            amounts: line.amounts,
        })
        .collect();
    Ok(LinkedOrderView {
        order_id: row.order_id,
        created_at: snapshot.identity.created_at,
        linked_at: row.linked_at,
        currency: snapshot.quote.currency,
        lines,
        shipping: snapshot.quote.shipping,
        total: snapshot.quote.total,
    })
}

/// Read an exact linked order while retaining the customer session claim.
pub async fn read_linked_order(
    pool: &PgPool,
    customer: &CustomerSessionToken,
    order_id: &str,
    clock: &dyn Clock,
) -> Result<LinkedOrderView, LinkError> {
    if !valid_hex(order_id, 32) {
        return Err(LinkError::Denied);
    }
    let observed = ObservedClock::new(clock, 0);
    time(&observed)?;
    let mut tx = customer_sessions::begin(pool).await?;
    let session = customer_sessions::claim_session(&mut tx, customer, &observed).await?;
    let account_id = session.account_id();
    let sql = "SELECT left(l.order_id,33) AS order_id,l.linked_at,left(p.basket_id,33) AS basket_id,p.created_at AS proof_created_at,p.expires_at AS proof_expires_at,COALESCE(octet_length(l.proof_verifier)=32 AND p.verifier=l.proof_verifier AND p.order_id=l.order_id AND p.consumed_account_id=l.account_id AND p.consumed_at=l.linked_at AND p.consumed_xid=l.linked_xid AND l.linked_xid IS NOT NULL,false) AS reciprocal FROM customer_order_links l LEFT JOIN guest_order_link_proofs p ON p.verifier=l.proof_verifier WHERE l.account_id=$1 AND l.order_id=$2";
    let row: Option<LinkRow> = sqlx::query_as(sql)
        .bind(account_id)
        .bind(order_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| LinkError::Unavailable)?;
    let row = row.ok_or(LinkError::Denied)?;
    let (email, verified_at) = account_email(&mut tx, account_id).await?;
    let at = time(&observed)?;
    let result = project(&mut tx, row, &email, verified_at, at).await?;
    session.check_at(time(&observed)?)?;
    tx.commit().await.map_err(|_| LinkError::Unavailable)?;
    Ok(result)
}

/// List an account's linked orders in descending link-time and C-collated ID order.
pub async fn list_linked_orders(
    pool: &PgPool,
    customer: &CustomerSessionToken,
    after: Option<&LinkedOrderCursor>,
    limit: u8,
    clock: &dyn Clock,
) -> Result<LinkedOrderPage, LinkError> {
    if !(1..=8).contains(&limit)
        || after.is_some_and(|c| !valid_timestamp(c.linked_at) || !valid_hex(&c.order_id, 32))
    {
        return Err(LinkError::Denied);
    }
    let observed = ObservedClock::new(clock, 0);
    time(&observed)?;
    let mut tx = customer_sessions::begin(pool).await?;
    let session = customer_sessions::claim_session(&mut tx, customer, &observed).await?;
    let account_id = session.account_id();
    let rows: Vec<LinkRow> = if let Some(cursor) = after {
        let sql = "SELECT left(l.order_id,33) AS order_id,l.linked_at,left(p.basket_id,33) AS basket_id,p.created_at AS proof_created_at,p.expires_at AS proof_expires_at,COALESCE(octet_length(l.proof_verifier)=32 AND p.verifier=l.proof_verifier AND p.order_id=l.order_id AND p.consumed_account_id=l.account_id AND p.consumed_at=l.linked_at AND p.consumed_xid=l.linked_xid AND l.linked_xid IS NOT NULL,false) AS reciprocal FROM customer_order_links l LEFT JOIN guest_order_link_proofs p ON p.verifier=l.proof_verifier WHERE l.account_id=$1 AND (l.linked_at,l.order_id COLLATE \"C\") < ($2,$3 COLLATE \"C\") ORDER BY l.linked_at DESC,l.order_id COLLATE \"C\" DESC LIMIT $4";
        sqlx::query_as(sql)
            .bind(account_id)
            .bind(cursor.linked_at)
            .bind(&cursor.order_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| LinkError::Unavailable)?
    } else {
        let sql = "SELECT left(l.order_id,33) AS order_id,l.linked_at,left(p.basket_id,33) AS basket_id,p.created_at AS proof_created_at,p.expires_at AS proof_expires_at,COALESCE(octet_length(l.proof_verifier)=32 AND p.verifier=l.proof_verifier AND p.order_id=l.order_id AND p.consumed_account_id=l.account_id AND p.consumed_at=l.linked_at AND p.consumed_xid=l.linked_xid AND l.linked_xid IS NOT NULL,false) AS reciprocal FROM customer_order_links l LEFT JOIN guest_order_link_proofs p ON p.verifier=l.proof_verifier WHERE l.account_id=$1 ORDER BY l.linked_at DESC,l.order_id COLLATE \"C\" DESC LIMIT $2";
        sqlx::query_as(sql)
            .bind(account_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| LinkError::Unavailable)?
    };
    let has_more = rows.len() > usize::from(limit);
    let (email, verified_at) = account_email(&mut tx, account_id).await?;
    let at = time(&observed)?;
    let mut orders = Vec::with_capacity(usize::from(limit));
    for row in rows.into_iter().take(usize::from(limit)) {
        orders.push(project(&mut tx, row, &email, verified_at, at).await?);
    }
    let next_cursor = if has_more {
        orders.last().map(|order| LinkedOrderCursor {
            linked_at: order.linked_at,
            order_id: order.order_id.clone(),
        })
    } else {
        None
    };
    session.check_at(time(&observed)?)?;
    tx.commit().await.map_err(|_| LinkError::Unavailable)?;
    Ok(LinkedOrderPage {
        orders,
        next_cursor,
    })
}
