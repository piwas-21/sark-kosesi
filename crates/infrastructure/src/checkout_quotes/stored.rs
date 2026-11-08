use super::*;
use commerce_domain::{
    checkout_quotes::PromotedQuoteSnapshot, promotion_policies::PromotionPolicy,
};

pub(crate) enum StoredQuote {
    Original(Box<QuoteSnapshot>),
    Promoted(Box<PromotedQuoteSnapshot>, Box<PromotionPolicy>),
}
type PromotionHeader = (Option<String>, Option<i32>, Option<i32>, bool);

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
    promotion_columns: bool,
    snapshot: Option<serde_json::Value>,
}
impl Row {
    fn binding(&self) -> QuoteBinding {
        QuoteBinding {
            quote_id: self.id.clone(),
            basket_id: self.basket_id.clone(),
            basket_revision: self.basket_revision,
            locale: self.locale.clone(),
            country: self.country.clone(),
            postcode: self.postcode.clone(),
            policy_id: self.policy_id.clone(),
            policy_revision: self.policy_revision,
            policy_review_revision: self.policy_review_revision,
            created_at: self.created_at,
            expires_at: self.expires_at,
        }
    }
    fn check_binding(&self, snapshot: &QuoteSnapshot) -> Result<(), QuoteStoreError> {
        if snapshot.binding != self.binding() || !valid_command_id(&self.effect_key) {
            return Err(QuoteStoreError::InvalidStoredData);
        }
        Ok(())
    }
    #[cfg(test)]
    fn decode(self) -> Result<(QuoteSnapshot, String), QuoteStoreError> {
        let snapshot = super::decode_snapshot(
            self.snapshot
                .clone()
                .ok_or(QuoteStoreError::InvalidStoredData)?,
        )?;
        self.check_binding(&snapshot)?;
        Ok((snapshot, self.effect_key))
    }
    async fn decode_any(
        self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<(StoredQuote, String), QuoteStoreError> {
        let value = self
            .snapshot
            .as_ref()
            .ok_or(QuoteStoreError::InvalidStoredData)?;
        // A bounded legacy header remains readable before migration 0049. If the
        // marker column exists, inspect the typed identity independently of JSON.
        let refs = if self.promotion_columns {
            let row: Option<PromotionHeader> = sqlx::query_as("SELECT left(promotion_id,66),promotion_revision,promotion_review_revision,promotion_transaction_id IS NOT NULL FROM guest_quote_snapshots WHERE id=$1 AND basket_id=$2")
                .bind(&self.id).bind(&self.basket_id).fetch_optional(&mut **tx).await?;
            Some(row.ok_or(QuoteStoreError::InvalidStoredData)?)
        } else {
            None
        };
        let decoded = if value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            == Some(2)
        {
            let original = super::decode_snapshot(
                value
                    .get("original")
                    .cloned()
                    .ok_or(QuoteStoreError::InvalidStoredData)?,
            )?;
            self.check_binding(&original)?;
            if !value.is_object()
                || !value
                    .get("promotion")
                    .is_some_and(serde_json::Value::is_object)
                || !value
                    .get("shipping")
                    .is_some_and(serde_json::Value::is_object)
                || !value.get("total").is_some_and(serde_json::Value::is_object)
                || !value
                    .get("lines")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|lines| {
                        lines.len() <= 20
                            && lines.iter().all(|line| {
                                line.is_object()
                                    && line
                                        .get("amounts")
                                        .is_some_and(serde_json::Value::is_object)
                            })
                    })
            {
                return Err(QuoteStoreError::InvalidStoredData);
            }
            let snapshot: PromotedQuoteSnapshot = serde_json::from_value(value.clone())
                .map_err(|_| QuoteStoreError::InvalidStoredData)?;
            let Some((Some(id), Some(revision), Some(review), true)) = refs else {
                return Err(QuoteStoreError::InvalidStoredData);
            };
            if snapshot.promotion.id != id
                || snapshot.promotion.revision != revision
                || snapshot.promotion.review_revision != review
            {
                return Err(QuoteStoreError::InvalidStoredData);
            }
            let policy =
                promotion_policies::historical_for_quote(tx, &id, revision, review).await?;
            snapshot.validate(&policy, review)?;
            StoredQuote::Promoted(Box::new(snapshot), Box::new(policy))
        } else {
            if !matches!(refs, None | Some((None, None, None, false))) {
                return Err(QuoteStoreError::InvalidStoredData);
            }
            let snapshot = super::decode_snapshot(value.clone())?;
            self.check_binding(&snapshot)?;
            StoredQuote::Original(Box::new(snapshot))
        };
        Ok((decoded, self.effect_key))
    }
}

pub(super) async fn by_effect(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    key: &str,
) -> Result<Option<(StoredQuote, String)>, QuoteStoreError> {
    let row = sqlx::query_as::<_, Row>("SELECT left(id,33) AS id,left(basket_id,33) AS basket_id,basket_revision,left(effect_key,129) AS effect_key,left(locale,4) AS locale,left(country,3) AS country,left(postcode,17) AS postcode,left(policy_id,66) AS policy_id,policy_revision,policy_review_revision,created_at,expires_at,EXISTS(SELECT 1 FROM pg_catalog.pg_attribute WHERE attrelid='guest_quote_snapshots'::regclass AND attname='promotion_transaction_id' AND attnum>0 AND NOT attisdropped) AS promotion_columns,CASE WHEN octet_length(snapshot::text)<=32768 THEN snapshot END AS snapshot FROM guest_quote_snapshots WHERE basket_id=$1 AND effect_key=$2").bind(owner).bind(key).fetch_optional(&mut **tx).await?;
    match row {
        Some(row) => Ok(Some(row.decode_any(tx).await?)),
        None => Ok(None),
    }
}
pub(crate) async fn by_id_any(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<StoredQuote, QuoteStoreError> {
    let row = sqlx::query_as::<_, Row>("SELECT left(id,33) AS id,left(basket_id,33) AS basket_id,basket_revision,left(effect_key,129) AS effect_key,left(locale,4) AS locale,left(country,3) AS country,left(postcode,17) AS postcode,left(policy_id,66) AS policy_id,policy_revision,policy_review_revision,created_at,expires_at,EXISTS(SELECT 1 FROM pg_catalog.pg_attribute WHERE attrelid='guest_quote_snapshots'::regclass AND attname='promotion_transaction_id' AND attnum>0 AND NOT attisdropped) AS promotion_columns,CASE WHEN octet_length(snapshot::text)<=32768 THEN snapshot END AS snapshot FROM guest_quote_snapshots WHERE basket_id=$1 AND id=$2").bind(owner).bind(id).fetch_optional(&mut **tx).await?.ok_or(QuoteStoreError::NotFound)?;
    Ok(row.decode_any(tx).await?.0)
}
pub(crate) async fn by_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    id: &str,
) -> Result<QuoteSnapshot, QuoteStoreError> {
    match by_id_any(tx, owner, id).await? {
        StoredQuote::Original(snapshot) => Ok(*snapshot),
        StoredQuote::Promoted(_, _) => Err(QuoteStoreError::NotFound),
    }
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
pub(super) async fn insert_promoted(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &PromotedQuoteSnapshot,
    effect: &str,
) -> Result<(), QuoteStoreError> {
    let b = &snapshot.original.binding;
    let json = serde_json::to_value(snapshot).map_err(|_| QuoteStoreError::InvalidStoredData)?;
    sqlx::query("INSERT INTO guest_quote_snapshots(id,basket_id,basket_revision,effect_key,locale,country,postcode,policy_id,policy_revision,policy_review_revision,created_at,expires_at,promotion_id,promotion_revision,promotion_review_revision,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)")
        .bind(&b.quote_id).bind(&b.basket_id).bind(b.basket_revision).bind(effect).bind(&b.locale).bind(&b.country).bind(&b.postcode).bind(&b.policy_id).bind(b.policy_revision).bind(b.policy_review_revision).bind(b.created_at).bind(b.expires_at).bind(&snapshot.promotion.id).bind(snapshot.promotion.revision).bind(snapshot.promotion.review_revision).bind(json).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/checkout_stored.rs"]
mod tests;
