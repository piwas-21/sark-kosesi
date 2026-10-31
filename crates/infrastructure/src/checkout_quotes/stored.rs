use super::*;
#[derive(sqlx::FromRow)]
pub(super) struct Row {
    id: String,
    basket_id: String,
    basket_revision: i32,
    effect_key: String,
    locale: String,
    country: String,
    postcode: String,
    policy_id: String,
    policy_revision: i32,
    policy_review_revision: i32,
    created_at: i64,
    expires_at: i64,
    snapshot: Option<serde_json::Value>,
}
impl Row {
    pub(super) fn decode(self) -> Result<(QuoteSnapshot, String), QuoteStoreError> {
        let value = self.snapshot.ok_or(QuoteStoreError::InvalidStoredData)?;
        let snapshot = super::decode_snapshot(value)?;
        let expected = QuoteBinding {
            quote_id: self.id,
            basket_id: self.basket_id,
            basket_revision: self.basket_revision,
            locale: self.locale,
            country: self.country,
            postcode: self.postcode,
            policy_id: self.policy_id,
            policy_revision: self.policy_revision,
            policy_review_revision: self.policy_review_revision,
            created_at: self.created_at,
            expires_at: self.expires_at,
        };
        if snapshot.binding != expected || !valid_command_id(&self.effect_key) {
            return Err(QuoteStoreError::InvalidStoredData);
        }
        Ok((snapshot, self.effect_key))
    }
}
pub(super) async fn by_effect(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    key: &str,
) -> Result<Option<(QuoteSnapshot, String)>, QuoteStoreError> {
    sqlx::query_as::<_, Row>("SELECT left(id,33) AS id,left(basket_id,33) AS basket_id,basket_revision,left(effect_key,129) AS effect_key,left(locale,4) AS locale,left(country,3) AS country,left(postcode,17) AS postcode,left(policy_id,66) AS policy_id,policy_revision,policy_review_revision,created_at,expires_at,CASE WHEN octet_length(snapshot::text)<=32768 THEN snapshot END AS snapshot FROM guest_quote_snapshots WHERE basket_id=$1 AND effect_key=$2")
        .bind(owner)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?
        .map(Row::decode)
        .transpose()
}
pub(crate) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<QuoteSnapshot, QuoteStoreError> {
    let row = sqlx::query_as::<_, Row>("SELECT left(id,33) AS id,left(basket_id,33) AS basket_id,basket_revision,left(effect_key,129) AS effect_key,left(locale,4) AS locale,left(country,3) AS country,left(postcode,17) AS postcode,left(policy_id,66) AS policy_id,policy_revision,policy_review_revision,created_at,expires_at,CASE WHEN octet_length(snapshot::text)<=32768 THEN snapshot END AS snapshot FROM guest_quote_snapshots WHERE basket_id=$1 AND id=$2")
        .bind(owner)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(QuoteStoreError::NotFound)?;
    Ok(row.decode()?.0)
}
pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &QuoteSnapshot,
    effect: &str,
) -> Result<(), QuoteStoreError> {
    let b = &snapshot.binding;
    let json = serde_json::to_value(snapshot).map_err(|_| QuoteStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO guest_quote_snapshots(id,basket_id,basket_revision,effect_key,locale,country,postcode,policy_id,policy_revision,policy_review_revision,created_at,expires_at,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
 .bind(&b.quote_id).bind(&b.basket_id).bind(b.basket_revision).bind(effect).bind(&b.locale).bind(&b.country).bind(&b.postcode).bind(&b.policy_id).bind(b.policy_revision).bind(b.policy_review_revision).bind(b.created_at).bind(b.expires_at).bind(json).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/checkout_stored.rs"]
mod tests;
