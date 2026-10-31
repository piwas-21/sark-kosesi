use super::review::StoredReview;
use super::*;
use crate::{published_catalogue, quotes};
use commerce_application::Locale;

#[derive(Debug)]
pub struct PolicyInspection {
    pub policy: MarketPolicy,
    pub author: String,
    pub reason: String,
    pub review: Option<ReviewInspection>,
}
#[derive(Debug)]
pub struct ReviewInspection {
    pub revision: i32,
    pub approved: bool,
    pub evidence_ref: String,
    pub qualification_ref: String,
    pub reviewer: String,
    pub reason: String,
}
async fn inspection(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    revision: i32,
) -> Result<PolicyInspection, MarketError> {
    let draft = load(tx, id, revision).await?;
    let policy = draft.decode(id, revision)?;
    let review: Option<StoredReview> = sqlx::query_as("SELECT draft_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM market_policy_reviews WHERE id=$1 AND draft_revision=$2 ORDER BY revision DESC LIMIT 1")
        .bind(id).bind(revision).fetch_optional(&mut **tx).await?;
    let review = if let Some(r) = review {
        if !r.valid(&draft) {
            return Err(MarketError::InvalidStoredData);
        }
        Some(ReviewInspection {
            revision: r.revision,
            approved: r.decision == "approved",
            evidence_ref: r.evidence_ref,
            qualification_ref: r.qualification_ref,
            reviewer: r.actor,
            reason: r.reason,
        })
    } else {
        None
    };
    Ok(PolicyInspection {
        policy,
        author: draft.actor,
        reason: draft.reason,
        review,
    })
}
/// Review-only source inspection, including historical drafts for withdrawal.
pub async fn inspect_policy(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    id: &str,
    revision: i32,
) -> Result<PolicyInspection, MarketError> {
    let mut access = lock_staff(pool, token, clock, Capability::MarketReview).await?;
    if !merchant_code(id) || revision <= 0 {
        return Err(MarketError::InvalidRequest);
    }
    let anchor: Option<String> =
        sqlx::query_scalar("SELECT id FROM market_policy_anchors WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut *access.tx)
            .await?;
    anchor.ok_or(MarketError::NotFound)?;
    let result = inspection(&mut access.tx, id, revision).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(result)
}
/// Only the newest draft with its newest independent approval can supply a quote.
/// Policy locks span catalogue/stock reads and final timed eligibility checks.
pub async fn reviewed_quote(
    pool: &PgPool,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
) -> Result<quotes::ServerQuote, MarketError> {
    request
        .validate()
        .map_err(|e| MarketError::Quote(e.into()))?;
    let mut tx = published_catalogue::begin(pool)
        .await
        .map_err(|e| MarketError::Quote(e.into()))?;
    let policy = reviewed_policy(&mut tx, request).await?;
    let quote = quotes::in_transaction(&mut tx, clock, locale, request, &[policy])
        .await
        .map_err(MarketError::Quote)?;
    tx.rollback().await?;
    Ok(quote)
}

async fn reviewed_policy(
    tx: &mut Transaction<'_, Postgres>,
    request: &QuoteRequest,
) -> Result<MarketPolicy, MarketError> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM market_policy_anchors WHERE country=$1 AND postcode=$2 FOR SHARE",
    )
    .bind(&request.destination.country)
    .bind(&request.destination.postcode)
    .fetch_optional(&mut **tx)
    .await?;
    let id = id.ok_or(MarketError::NotFound)?;
    let revision: Option<i32> =
        sqlx::query_scalar("SELECT max(revision) FROM market_policy_drafts WHERE id=$1")
            .bind(&id)
            .fetch_one(&mut **tx)
            .await?;
    let policy = inspection(tx, &id, revision.ok_or(MarketError::NotFound)?).await?;
    if !policy.review.is_some_and(|r| r.approved) {
        return Err(MarketError::NotFound);
    }
    Ok(policy.policy)
}
/// Indicative trusted server terms only; no coupon HTTP route or budget claim.
pub async fn reviewed_promotion_quote(
    pool: &PgPool,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    promotion: &commerce_domain::promotions::Promotion,
) -> Result<quotes::PromotedServerQuote, MarketError> {
    request
        .validate()
        .map_err(|e| MarketError::Quote(e.into()))?;
    let mut tx = published_catalogue::begin(pool)
        .await
        .map_err(|e| MarketError::Quote(e.into()))?;
    let policy = reviewed_policy(&mut tx, request).await?;
    let quote =
        quotes::promoted_in_transaction(&mut tx, clock, locale, request, &[policy], promotion)
            .await
            .map_err(MarketError::Quote)?;
    tx.rollback().await?;
    Ok(quote)
}
