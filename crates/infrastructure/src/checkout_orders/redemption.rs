use super::*;
use commerce_domain::{
    checkout_quotes::PromotedQuoteSnapshot, promotion_policies::PromotionPolicy,
};

/// Market precedes this exclusive promotion claim; catalogue and stock follow it.
pub(super) async fn claim(
    scope: &mut BasketScope,
    promoted: Option<&PromotedQuoteSnapshot>,
    account: Option<i64>,
) -> Result<Option<PromotionPolicy>, OrderStoreError> {
    let Some(p) = promoted else {
        return Ok(None);
    };
    let (current, review) =
        crate::promotion_policies::current_for_redemption(scope.transaction(), &p.promotion.id)
            .await
            .map_err(|e| match e {
                crate::promotion_policies::PolicyError::Database(_) => OrderStoreError::Database,
                crate::promotion_policies::PolicyError::InvalidStoredData => {
                    OrderStoreError::InvalidStoredData
                }
                _ => OrderStoreError::Unavailable,
            })?;
    if current.promotion.revision != p.promotion.revision || review != p.promotion.review_revision {
        return Err(OrderStoreError::QuoteChanged);
    }
    // Reconstruct the approved allocation with the authority held for this claim.
    let at = scope.checked_at()?;
    if account.is_some() {
        p.check_at_verified(&current, review, at)?;
    } else {
        p.check_at(&current, review, at)?;
    }
    if let Some(limit) = current.limits.per_customer_redemptions {
        let account = account.ok_or(OrderStoreError::Unavailable)?;
        let (head, anonymous): (Option<i64>, bool) = sqlx::query_as(
            "SELECT (SELECT account_redemption_number FROM promotion_redemptions WHERE promotion_id=$1 AND promotion_account_id=$2 ORDER BY account_redemption_number DESC LIMIT 1), EXISTS(SELECT 1 FROM promotion_redemptions WHERE promotion_id=$1 AND promotion_account_id IS NULL)",
        )
        .bind(&p.promotion.id)
        .bind(account)
        .fetch_one(&mut **scope.transaction())
        .await?;
        let used = match head {
            Some(number) if (1..=1_000_000).contains(&number) => number,
            Some(_) => return Err(OrderStoreError::InvalidStoredData),
            None => 0,
        };
        if anonymous || used >= i64::from(limit) {
            return Err(OrderStoreError::Unavailable);
        }
    }
    let latest: Option<(i64, i64)> = sqlx::query_as(
        "SELECT redemption_number,lifetime_discount_minor FROM promotion_redemptions WHERE promotion_id=$1 ORDER BY redemption_number DESC LIMIT 1",
    ).bind(&p.promotion.id).fetch_optional(&mut **scope.transaction()).await?;
    let (count, spent) = match latest {
        Some((count, spent))
            if (1..=1_000_000).contains(&count)
                && (1..=commerce_domain::quotes::MAX_MINOR_UNITS).contains(&spent) =>
        {
            (count, spent)
        }
        Some(_) => return Err(OrderStoreError::InvalidStoredData),
        None => (0, 0),
    };
    let next_spent = spent
        .checked_add(p.discount_minor)
        .ok_or(OrderStoreError::Unavailable)?;
    if count >= i64::from(current.limits.total_redemptions)
        || next_spent > current.limits.total_discount_minor
    {
        return Err(OrderStoreError::Unavailable);
    }
    Ok(Some(current))
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &OrderSnapshot,
    account: Option<i64>,
) -> Result<(), OrderStoreError> {
    if let Some(p) = &snapshot.promoted_quote {
        let query = if account.is_some() {
            "INSERT INTO promotion_redemptions(order_id,quote_id,promotion_id,promotion_revision,promotion_review_revision,discount_minor,created_at,promotion_account_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)"
        } else {
            "INSERT INTO promotion_redemptions(order_id,quote_id,promotion_id,promotion_revision,promotion_review_revision,discount_minor,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)"
        };
        let query = sqlx::query(query)
            .bind(&snapshot.identity.id)
            .bind(&snapshot.identity.quote_id)
            .bind(&p.promotion.id)
            .bind(p.promotion.revision)
            .bind(p.promotion.review_revision)
            .bind(p.discount_minor)
            .bind(snapshot.identity.created_at);
        let query = if let Some(account) = account {
            query.bind(account)
        } else {
            query
        };
        query.execute(&mut **tx).await?;
    }
    Ok(())
}
