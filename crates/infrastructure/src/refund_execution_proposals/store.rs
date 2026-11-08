use super::*;
use sqlx::{Postgres, Transaction};

type Tx<'a> = Transaction<'a, Postgres>;

#[derive(sqlx::FromRow)]
pub(super) struct Row {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub reservation_id: String,
    pub recognition_id: String,
    pub through_observation: i64,
    pub actor_id: String,
    pub reason: String,
    pub proposed_at: i64,
    allocation: Option<serde_json::Value>,
}
impl Row {
    pub fn receipt(self) -> Result<ProposalReceipt, ProposalError> {
        if ![
            &self.id,
            &self.order_id,
            &self.attempt_id,
            &self.reservation_id,
            &self.recognition_id,
        ]
        .iter()
        .all(|id| identifier(id))
            || !commerce_domain::media::valid_command_id(&self.actor_id)
            || !(0..=1000).contains(&self.through_observation)
            || self.proposed_at < 0
            || self.proposed_at > 9_007_199_254_740_991
            || self.reason.is_empty()
            || self.reason.len() > 240
            || self.reason.trim() != self.reason
            || self.reason.chars().any(char::is_control)
        {
            return Err(ProposalError::InvalidStoredData);
        }
        let allocation: RefundAllocation =
            serde_json::from_value(self.allocation.ok_or(ProposalError::InvalidStoredData)?)
                .map_err(|_| ProposalError::InvalidStoredData)?;
        if allocation.schema_version != 1 || !(1..=20).contains(&allocation.lines.len()) {
            return Err(ProposalError::InvalidStoredData);
        }
        Ok(ProposalReceipt {
            id: self.id,
            order_id: self.order_id,
            reservation_id: self.reservation_id,
            recognition_id: self.recognition_id,
            through_observation: self.through_observation,
            allocation,
            proposed_at: self.proposed_at,
            replayed: false,
        })
    }
}
pub(super) async fn lookup(
    tx: &mut Tx<'_>,
    key: &str,
    id: Option<&str>,
) -> Result<Option<Row>, ProposalError> {
    Ok(sqlx::query_as("SELECT left(id,33) id,left(order_id,33) order_id,left(attempt_id,33) attempt_id,left(reservation_id,33) reservation_id,left(recognition_id,33) recognition_id,through_observation,left(actor_id,129) actor_id,left(reason,241) reason,proposed_at,CASE WHEN octet_length(allocation::text)<=16384 THEN allocation END allocation FROM refund_execution_proposals WHERE effect_key=$1 AND ($2::text IS NULL OR id=$2)")
        .bind(key).bind(id).fetch_optional(&mut **tx).await?)
}
pub(super) async fn validate(tx: &mut Tx<'_>, id: &str) -> Result<(), ProposalError> {
    let valid: bool = sqlx::query_scalar("SELECT COALESCE((SELECT refund_execution_proposal_source(p,false) AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.id=p.event_id AND e.aggregate_id=p.order_id AND e.kind='order.refund_execution_proposed.v1' AND e.created_at=p.proposed_at AND e.payload=refund_execution_proposal_event(p)) FROM refund_execution_proposals p WHERE p.id=$1),false)")
        .bind(id).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(ProposalError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn reservation_used(
    tx: &mut Tx<'_>,
    reservation: &str,
    attempt: &str,
) -> Result<bool, ProposalError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM refund_execution_proposals WHERE reservation_id=$1 AND attempt_id=$2)",
    )
    .bind(reservation)
    .bind(attempt)
    .fetch_one(&mut **tx)
    .await?)
}
#[derive(sqlx::FromRow)]
pub(super) struct Candidate {
    attempt_id: String,
    reservation_sha256: String,
    profile_id: String,
    payment_id: String,
    allocation: serde_json::Value,
    pub minimum_time: i64,
}
pub(super) async fn candidate(
    tx: &mut Tx<'_>,
    attempt: &str,
    r: &ProposalRequest<'_>,
) -> Result<Candidate, ProposalError> {
    // All values below come from captured and recognized source records; the native
    // completion constraint validates their full reciprocal custody after insertion.
    sqlx::query_as("SELECT a.id attempt_id,refund_reservation_digest(s) reservation_sha256,a.configuration->'profile'->>'id' profile_id,i.summary->'paid_provider_ids'->>0 payment_id,s.allocation,GREATEST(s.reserved_at,n.recorded_at) minimum_time FROM payment_attempts a JOIN refund_reservations s ON s.attempt_id=a.id JOIN payment_recognition_current c ON c.order_id=a.order_id JOIN payment_recognitions n ON n.id=c.recognition_id AND n.attempt_id=a.id JOIN payment_identity_resolutions i ON i.id=n.resolution_id WHERE a.id=$1 AND a.order_id=$2 AND s.id=$3 AND n.id=$4 AND n.through_observation=$5 AND n.state='received' AND jsonb_array_length(i.summary->'provider_ids')=1 AND jsonb_array_length(i.summary->'paid_provider_ids')=1 AND i.summary->'provider_ids'->>0=i.summary->'paid_provider_ids'->>0 AND NOT EXISTS(SELECT 1 FROM payment_observations o WHERE o.attempt_id=a.id AND o.revision>n.through_observation)")
        .bind(attempt).bind(r.order_id).bind(r.reservation_id).bind(r.expected_recognition_id)
        .bind(r.expected_through_observation).fetch_optional(&mut **tx).await?
        .ok_or(ProposalError::Unavailable)
}
pub(super) async fn insert(
    tx: &mut Tx<'_>,
    r: &ProposalRequest<'_>,
    c: &Candidate,
    id: &str,
    event: &str,
    actor: &str,
    at: i64,
) -> Result<(), ProposalError> {
    sqlx::query("INSERT INTO refund_execution_proposals(id,order_id,attempt_id,reservation_id,reservation_sha256,recognition_id,through_observation,profile_id,payment_id,actor_id,effect_key,reason,proposed_at,allocation,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(id).bind(r.order_id).bind(&c.attempt_id).bind(r.reservation_id).bind(&c.reservation_sha256)
        .bind(r.expected_recognition_id).bind(r.expected_through_observation).bind(&c.profile_id)
        .bind(&c.payment_id).bind(actor).bind(r.effect_key).bind(r.reason).bind(at)
        .bind(&c.allocation).bind(event).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT p.event_id,p.order_id,'order.refund_execution_proposed.v1',refund_execution_proposal_event(p),p.proposed_at FROM refund_execution_proposals p WHERE p.id=$1")
        .bind(id).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "0123456789abcdef0123456789abcdef";
    fn row() -> Row {
        Row {
            id: ID.into(),
            order_id: ID.into(),
            attempt_id: ID.into(),
            reservation_id: ID.into(),
            recognition_id: ID.into(),
            through_observation: 1,
            actor_id: "staff-1".into(),
            reason: "Requested".into(),
            proposed_at: 10,
            allocation: Some(
                serde_json::json!({"schema_version":1,"lines":[{"sku":"sku","gross_minor":100,"net_minor":83,"vat_minor":17}],"shipping":{"gross_minor":0,"net_minor":0,"vat_minor":0},"total":{"gross_minor":100,"net_minor":83,"vat_minor":17}}),
            ),
        }
    }
    #[test]
    fn bounded_closed_receipt_decode() {
        let decoded = row().receipt().expect("valid source");
        assert_eq!(decoded.allocation.total.gross_minor, 100);
        assert!(!decoded.replayed);
        let mut bad = row();
        bad.allocation = None;
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        let mut bad = row();
        bad.allocation = Some(serde_json::json!({"schema_version":1,"lines":[]}));
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        let mut bad = row();
        bad.id = "wrong".into();
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        let mut bad = row();
        bad.through_observation = 1001;
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        let mut bad = row();
        bad.reason = " bad".into();
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        let mut bad = row();
        bad.actor_id = "invalid actor".into();
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        for reason in ["", "bad\nreason", &"x".repeat(241)] {
            let mut bad = row();
            bad.reason = reason.into();
            assert!(matches!(
                bad.receipt(),
                Err(ProposalError::InvalidStoredData)
            ));
        }
        let mut bad = row();
        bad.proposed_at = 9_007_199_254_740_992;
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
        for (version, lines) in [(2, true), (1, false)] {
            let mut bad = row();
            let allocation = bad.allocation.as_mut().expect("fixture allocation");
            allocation["schema_version"] = version.into();
            if !lines {
                allocation["lines"] = serde_json::json!([]);
            }
            assert!(matches!(
                bad.receipt(),
                Err(ProposalError::InvalidStoredData)
            ));
        }
        let mut bad = row();
        bad.proposed_at = -1;
        assert!(matches!(
            bad.receipt(),
            Err(ProposalError::InvalidStoredData)
        ));
    }
}
