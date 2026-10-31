use super::review::StoredReview;
use super::*;
#[derive(Debug)]
pub struct PolicyInspection {
    pub policy: PromotionPolicy,
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
#[derive(Debug)]
pub struct ApprovedPolicy {
    pub policy: PromotionPolicy,
    pub review_revision: i32,
    pub evaluated_at: i64,
}
async fn inspection(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    revision: i32,
) -> Result<PolicyInspection, PolicyError> {
    let draft = load(tx, id, revision).await?;
    let policy = draft.decode(id, revision)?;
    let review:Option<StoredReview>=sqlx::query_as("SELECT draft_revision,revision,left(decision,9) AS decision,left(evidence_ref,501) AS evidence_ref,left(qualification_ref,501) AS qualification_ref,left(actor,501) AS actor,left(reason,501) AS reason FROM promotion_policy_reviews WHERE id=$1 AND draft_revision=$2 ORDER BY revision DESC LIMIT 1").bind(id).bind(revision).fetch_optional(&mut **tx).await?;
    let review = if let Some(r) = review {
        if r.draft_revision != revision || !r.valid(&draft) {
            return Err(PolicyError::InvalidStoredData);
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
async fn anchor(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<(), PolicyError> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT id FROM promotion_policy_anchors WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    found.ok_or(PolicyError::NotFound)?;
    Ok(())
}
/// Review-only inspection includes historical drafts so their approval can be withdrawn.
pub async fn inspect_policy(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    id: &str,
    revision: i32,
) -> Result<PolicyInspection, PolicyError> {
    let observed = ObservedClock::new(clock, 0);
    let clock: &dyn Clock = &observed;
    let mut access = lock_staff(pool, token, clock, Capability::PromotionReview).await?;
    if !merchant_code(id) || revision <= 0 {
        return Err(PolicyError::InvalidRequest);
    }
    anchor(&mut access.tx, id).await?;
    let result = inspection(&mut access.tx, id, revision).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(result)
}
async fn approved_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<PolicyInspection, PolicyError> {
    anchor(tx, id).await?;
    let revision: Option<i32> =
        sqlx::query_scalar("SELECT max(revision) FROM promotion_policy_drafts WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    let result = inspection(tx, id, revision.ok_or(PolicyError::NotFound)?).await?;
    if !result.review.as_ref().is_some_and(|r| r.approved) {
        return Err(PolicyError::NotFound);
    }
    Ok(result)
}
/// Trusted indicative configuration read, not a current quote or budget claim.
/// Order composition must retain this anchor claim with all other order sources.
pub async fn approved_policy(
    pool: &PgPool,
    clock: &dyn Clock,
    id: &str,
    country: &str,
) -> Result<ApprovedPolicy, PolicyError> {
    if !merchant_code(id) || !commerce_domain::quotes::valid_country(country) {
        return Err(PolicyError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let clock: &dyn Clock = &observed;
    let _started = now(clock)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    let result = approved_in_transaction(&mut tx, id).await?;
    let evaluated_at = now(clock)?;
    result
        .policy
        .permits(country, evaluated_at)
        .map_err(|_| PolicyError::NotFound)?;
    let review = result.review.ok_or(PolicyError::InvalidStoredData)?;
    tx.rollback().await?;
    Ok(ApprovedPolicy {
        policy: result.policy,
        review_revision: review.revision,
        evaluated_at,
    })
}
