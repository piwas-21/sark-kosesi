use super::*;
#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    case_id: String,
    case_digest: String,
    order_id: String,
    attempt_id: String,
    recognition_id: String,
    actor_id: String,
    effect_key: String,
    generation: i32,
    reason: String,
    recorded_at: i64,
    outcome: String,
    allocation_id: Option<String>,
    valid: bool,
}
impl Row {
    fn receipt(self) -> Result<DecisionReceipt, LateStockError> {
        if !self.valid
            || ![
                &self.id,
                &self.case_id,
                &self.order_id,
                &self.attempt_id,
                &self.recognition_id,
            ]
            .iter()
            .all(|s| identifier(s))
            || !commerce_domain::media::valid_command_id(&self.actor_id)
            || !hex(&self.case_digest, 64)
            || !hex(&self.effect_key, 32)
            || !(1..=3).contains(&self.generation)
            || self.reason.is_empty()
            || self.reason.len() > 240
            || self.reason.trim() != self.reason
        {
            return Err(LateStockError::InvalidStoredData);
        }
        let outcome = match (self.outcome.as_str(), self.allocation_id) {
            ("allocated", Some(id)) if identifier(&id) => {
                DecisionOutcome::Allocated { allocation_id: id }
            }
            ("still_scarce", None) => DecisionOutcome::StillScarce,
            _ => return Err(LateStockError::InvalidStoredData),
        };
        Ok(DecisionReceipt {
            id: self.id,
            case_id: self.case_id,
            case_digest: self.case_digest,
            order_id: self.order_id,
            attempt_id: self.attempt_id,
            recognition_id: self.recognition_id,
            actor_id: self.actor_id,
            effect_key: self.effect_key,
            generation: self.generation,
            reason: self.reason,
            recorded_at: self.recorded_at,
            outcome,
            replayed: false,
        })
    }
}
pub(super) async fn by_key(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
) -> Result<Option<DecisionReceipt>, LateStockError> {
    sqlx::query_as::<_, Row>("SELECT left(s.id,33) id,left(s.case_id,33) case_id,left(s.case_digest,65) case_digest,left(s.order_id,33) order_id,left(s.attempt_id,33) attempt_id,left(s.recognition_id,33) recognition_id,left(s.actor_id,129) actor_id,left(s.effect_key,33) effect_key,s.generation,left(s.reason,241) reason,s.recorded_at,left(s.outcome,13) outcome,left(s.allocation_id,33) allocation_id,true AS valid FROM late_stock_dispositions s WHERE s.effect_key=$1")
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?
        .map(Row::receipt)
        .transpose()
}
pub(in crate::late_stock) async fn validate_history(
    tx: &mut Transaction<'_, Postgres>,
    case: &str,
) -> Result<Vec<DecisionReceipt>, LateStockError> {
    let rows = sqlx::query_as::<_, Row>("WITH candidates AS MATERIALIZED (SELECT id,generation FROM late_stock_dispositions WHERE case_id=$1 ORDER BY generation LIMIT 4), proof AS MATERIALIZED (SELECT CASE WHEN (SELECT count(*) FROM candidates) BETWEEN 1 AND 3 THEN (SELECT late_stock_disposition_history_exact(d) FROM late_stock_dispositions d JOIN (SELECT id FROM candidates ORDER BY generation DESC LIMIT 1) latest ON latest.id=d.id) ELSE false END AS valid) SELECT left(s.id,33) id,left(s.case_id,33) case_id,left(s.case_digest,65) case_digest,left(s.order_id,33) order_id,left(s.attempt_id,33) attempt_id,left(s.recognition_id,33) recognition_id,left(s.actor_id,129) actor_id,left(s.effect_key,33) effect_key,s.generation,left(s.reason,241) reason,s.recorded_at,left(s.outcome,13) outcome,left(s.allocation_id,33) allocation_id,proof.valid FROM candidates c JOIN late_stock_dispositions s ON s.id=c.id CROSS JOIN proof ORDER BY s.generation")
        .bind(case)
        .fetch_all(&mut **tx)
        .await?;
    if rows.len() > 3 {
        return Err(LateStockError::InvalidStoredData);
    }
    let result = rows
        .into_iter()
        .map(Row::receipt)
        .collect::<Result<Vec<_>, _>>()?;
    for (index, item) in result.iter().enumerate() {
        if item.case_id != case
            || usize::try_from(item.generation).ok() != Some(index + 1)
            || (index + 1 < result.len()
                && matches!(item.outcome, DecisionOutcome::Allocated { .. }))
        {
            return Err(LateStockError::InvalidStoredData);
        }
    }
    Ok(result)
}
pub(in crate::late_stock) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    receipt: &DecisionReceipt,
    footprint: Option<&ReviewFootprint>,
    event: &str,
) -> Result<(), LateStockError> {
    let (outcome, allocation) = match &receipt.outcome {
        DecisionOutcome::Allocated { allocation_id } => ("allocated", Some(allocation_id.as_str())),
        DecisionOutcome::StillScarce => ("still_scarce", None),
    };
    let footprint = footprint.map(rows::envelope).transpose()?;
    let decision_insert = sqlx::query("INSERT INTO late_stock_dispositions(id,case_id,case_digest,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,recorded_at,outcome,allocation_id,footprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
        .bind(&receipt.id).bind(&receipt.case_id).bind(&receipt.case_digest).bind(&receipt.order_id).bind(&receipt.attempt_id).bind(&receipt.recognition_id).bind(&receipt.actor_id).bind(&receipt.effect_key).bind(receipt.generation).bind(&receipt.reason).bind(receipt.recorded_at).bind(outcome).bind(allocation).bind(footprint).execute(&mut **tx).await?;
    if decision_insert.rows_affected() != 1 {
        return Err(LateStockError::InvalidStoredData);
    }
    let event_insert = sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,s.order_id,'order.stock_recovery_decided.v1',jsonb_build_object('schema_version',1,'order_id',s.order_id,'decision_id',s.id,'decision_sha256',late_stock_disposition_digest(s)),s.recorded_at FROM late_stock_dispositions s WHERE s.id=$2")
        .bind(event).bind(&receipt.id).execute(&mut **tx).await?;
    if event_insert.rows_affected() != 1 {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
