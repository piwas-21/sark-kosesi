use super::*;
use commerce_domain::factual_review::FactDecision;

pub struct ReviewRequest<'a> {
    pub id: &'a str,
    pub draft_revision: i32,
    pub decision: FactDecision,
    pub evidence_ref: &'a str,
    pub qualification_ref: &'a str,
    pub audit: Audit<'a>,
}
#[derive(sqlx::FromRow)]
pub(super) struct StoredReview {
    pub draft_revision: i32,
    pub revision: i32,
    pub decision: String,
    pub evidence_ref: String,
    pub qualification_ref: String,
    pub actor: String,
    pub reason: String,
}
impl StoredReview {
    pub fn valid(&self, draft: &StoredDraft) -> bool {
        self.draft_revision > 0
            && self.revision > 0
            && ["approved", "rejected"].contains(&self.decision.as_str())
            && text(&self.actor)
            && self.actor != draft.actor
            && text(&self.evidence_ref)
            && text(&self.qualification_ref)
            && text(&self.reason)
    }
}
pub async fn record_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: ReviewRequest<'_>,
) -> Result<Receipt, PolicyError> {
    let observed = ObservedClock::new(clock, 0);
    let clock: &dyn Clock = &observed;
    let mut access = lock_staff(pool, token, clock, Capability::PromotionReview).await?;
    let audit = request.audit;
    if !audit.valid()
        || !merchant_code(request.id)
        || request.draft_revision <= 0
        || !text(request.evidence_ref)
        || !text(request.qualification_ref)
    {
        return Err(PolicyError::InvalidRequest);
    }
    effect_lock(&mut access.tx, "review", audit.effect_key).await?;
    let prior: Option<(String, i32)> = sqlx::query_as(
        "SELECT left(id,66),revision FROM promotion_policy_reviews WHERE effect_key=$1",
    )
    .bind(audit.effect_key)
    .fetch_optional(&mut *access.tx)
    .await?;
    let current = claim(&mut access.tx, request.id).await?;
    let draft = load(&mut access.tx, request.id, request.draft_revision).await?;
    draft.decode(request.id, request.draft_revision)?;
    if draft.actor == access.actor {
        return Err(PolicyError::SelfReview);
    }
    if let Some((id, revision)) = prior {
        let stored: StoredReview = sqlx::query_as("SELECT draft_revision,revision,left(decision,9) AS decision,left(evidence_ref,501) AS evidence_ref,left(qualification_ref,501) AS qualification_ref,left(actor,501) AS actor,left(reason,501) AS reason FROM promotion_policy_reviews WHERE effect_key=$1")
            .bind(audit.effect_key).fetch_one(&mut *access.tx).await?;
        if id != request.id
            || stored.draft_revision != request.draft_revision
            || revision.checked_sub(1) != Some(audit.expected_revision)
            || stored.decision != request.decision.code()
            || stored.evidence_ref != request.evidence_ref
            || stored.qualification_ref != request.qualification_ref
            || stored.actor != access.actor
            || stored.reason != audit.reason
        {
            return Err(PolicyError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(Receipt {
            revision,
            replayed: true,
        });
    }
    let review: i32 = sqlx::query_scalar("SELECT coalesce(max(revision),0) FROM promotion_policy_reviews WHERE id=$1 AND draft_revision=$2")
        .bind(request.id).bind(request.draft_revision).fetch_one(&mut *access.tx).await?;
    if review != audit.expected_revision
        || (request.decision == FactDecision::Approved && current != request.draft_revision)
    {
        return Err(PolicyError::Conflict);
    }
    let revision = audit
        .expected_revision
        .checked_add(1)
        .ok_or(PolicyError::InvalidRequest)?;
    sqlx::query("INSERT INTO promotion_policy_reviews(id,draft_revision,revision,decision,evidence_ref,qualification_ref,effect_key,actor,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(request.id).bind(request.draft_revision).bind(revision).bind(request.decision.code())
        .bind(request.evidence_ref).bind(request.qualification_ref).bind(audit.effect_key)
        .bind(&access.actor).bind(audit.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(Receipt {
        revision,
        replayed: false,
    })
}
