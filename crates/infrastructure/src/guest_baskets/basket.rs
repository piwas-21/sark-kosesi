use super::*;
use commerce_domain::media::valid_command_id;
#[derive(sqlx::FromRow)]
struct BasketRow {
    revision: i32,
    lines: Option<serde_json::Value>,
    last_effect_key: Option<String>,
    rate_window: i64,
    mutation_count: i32,
    head_revision: Option<i32>,
    head_expected: Option<i32>,
    head_lines: Option<serde_json::Value>,
}
fn lines(value: &Option<serde_json::Value>) -> Result<Vec<BasketLine>, GuestError> {
    let value = value.as_ref().ok_or(GuestError::InvalidStoredData)?;
    let mut result: Vec<BasketLine> =
        serde_json::from_value(value.clone()).map_err(|_| GuestError::InvalidStoredData)?;
    if !valid_lines(&result) {
        return Err(GuestError::InvalidStoredData);
    }
    result.sort_by(|a, b| a.sku.cmp(&b.sku));
    Ok(result)
}
impl BasketRow {
    fn validated(&self) -> Result<Vec<BasketLine>, GuestError> {
        if self.revision < 0 || self.rate_window < 0 || !(0..=1000).contains(&self.mutation_count) {
            return Err(GuestError::InvalidStoredData);
        }
        let selected = lines(&self.lines)?;
        if self.revision == 0 {
            if !selected.is_empty()
                || self.last_effect_key.is_some()
                || self.head_revision.is_some()
            {
                return Err(GuestError::InvalidStoredData);
            }
        } else if !self
            .last_effect_key
            .as_deref()
            .is_some_and(valid_command_id)
            || self.head_revision != Some(self.revision)
            || self.head_expected != self.revision.checked_sub(1)
            || self.head_lines != self.lines
        {
            return Err(GuestError::InvalidStoredData);
        }
        Ok(selected)
    }
}
async fn load(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    write: bool,
) -> Result<BasketRow, GuestError> {
    let sql = if write {
        "SELECT id FROM guest_baskets WHERE id=$1 FOR UPDATE"
    } else {
        "SELECT id FROM guest_baskets WHERE id=$1 FOR SHARE"
    };
    let id: Option<String> = sqlx::query_scalar(sql)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    let id = id.ok_or(GuestError::InvalidStoredData)?;
    Ok(sqlx::query_as("SELECT b.revision,CASE WHEN octet_length(b.lines::text)<=8192 THEN b.lines END AS lines,b.last_effect_key,b.rate_window,b.mutation_count,e.revision AS head_revision,e.expected_revision AS head_expected,CASE WHEN octet_length(e.lines::text)<=8192 THEN e.lines END AS head_lines FROM guest_baskets b LEFT JOIN guest_basket_effects e ON e.basket_id=b.id AND e.effect_key=b.last_effect_key WHERE b.id=$1")
        .bind(id).fetch_one(&mut **tx).await?)
}
pub async fn read_basket(
    store: &GuestSessionStore,
    key: &GuestCredential,
) -> Result<BasketView, GuestError> {
    let mut access = access::lock(store, key, None).await?;
    let row = load(&mut access.tx, &access.identity.basket_id, false).await?;
    let lines = row.validated()?;
    let at = access.checked_at(store.clock.as_ref())?;
    if row.rate_window > at / 60 {
        return Err(GuestError::ClockUnavailable);
    }
    let view = BasketView {
        revision: row.revision,
        lines,
        csrf_token: access.identity.csrf_token,
    };
    access.tx.rollback().await?;
    Ok(view)
}
#[derive(sqlx::FromRow)]
struct Effect {
    expected_revision: i32,
    revision: i32,
    lines: Option<serde_json::Value>,
}
impl Effect {
    fn validated(&self) -> Result<Vec<BasketLine>, GuestError> {
        if !(0..i32::MAX).contains(&self.expected_revision)
            || self.expected_revision.checked_add(1) != Some(self.revision)
        {
            return Err(GuestError::InvalidStoredData);
        }
        lines(&self.lines)
    }
}
pub async fn write_basket(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    request: &BasketUpdate,
) -> Result<BasketReceipt, GuestError> {
    let mut access = access::lock(store, key, Some(csrf)).await?;
    let selected = request
        .canonical_lines()
        .map_err(|_| GuestError::InvalidRequest)?;
    let row = load(&mut access.tx, &access.identity.basket_id, true).await?;
    row.validated()?;
    let prior:Option<Effect>=sqlx::query_as("SELECT expected_revision,revision,CASE WHEN octet_length(lines::text)<=8192 THEN lines END AS lines FROM guest_basket_effects WHERE basket_id=$1 AND effect_key=$2")
        .bind(&access.identity.basket_id).bind(&request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(effect) = prior {
        let original = effect.validated()?;
        if effect.expected_revision != request.expected_revision || original != selected {
            return Err(GuestError::Conflict);
        }
        access.checked_at(store.clock.as_ref())?;
        access.tx.rollback().await?;
        return Ok(BasketReceipt {
            revision: effect.revision,
            replayed: true,
        });
    }
    if row.revision != request.expected_revision {
        return Err(GuestError::Conflict);
    }
    let at = access.checked_at(store.clock.as_ref())?;
    let window = at / 60;
    if window < row.rate_window {
        return Err(GuestError::ClockUnavailable);
    }
    let count = if window == row.rate_window {
        row.mutation_count
    } else {
        0
    };
    if count >= i32::from(store.limits.mutations_per_minute) {
        return Err(GuestError::RateLimited);
    }
    let count = count.checked_add(1).ok_or(GuestError::InvalidStoredData)?;
    let revision = row
        .revision
        .checked_add(1)
        .ok_or(GuestError::InvalidStoredData)?;
    let payload = serde_json::to_value(&selected).map_err(|_| GuestError::InvalidRequest)?;
    sqlx::query("INSERT INTO guest_basket_effects(basket_id,effect_key,expected_revision,revision,lines) VALUES($1,$2,$3,$4,$5)")
        .bind(&access.identity.basket_id).bind(&request.effect_key).bind(request.expected_revision).bind(revision).bind(&payload).execute(&mut *access.tx).await?;
    sqlx::query("UPDATE guest_baskets SET revision=$2,lines=$3,last_effect_key=$4,rate_window=$5,mutation_count=$6 WHERE id=$1")
        .bind(&access.identity.basket_id).bind(revision).bind(payload).bind(&request.effect_key).bind(window).bind(count).execute(&mut *access.tx).await?;
    access.checked_at(store.clock.as_ref())?;
    access.tx.commit().await?;
    Ok(BasketReceipt {
        revision,
        replayed: false,
    })
}
pub(super) async fn locked_intent(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    write: bool,
) -> Result<(i32, Vec<BasketLine>, i64), GuestError> {
    let row = load(tx, id, write).await?;
    Ok((row.revision, row.validated()?, row.rate_window))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(revision: i32) -> BasketRow {
        BasketRow {
            revision,
            lines: Some(serde_json::json!([])),
            last_effect_key: (revision > 0).then(|| "one".into()),
            rate_window: 0,
            mutation_count: 0,
            head_revision: (revision > 0).then_some(revision),
            head_expected: (revision > 0).then_some(0),
            head_lines: (revision > 0).then(|| serde_json::json!([])),
        }
    }
    #[test]
    fn damaged_snapshot_headers_and_incomplete_or_changed_receipts_cannot_supply_intent() {
        assert!(row(0).validated().expect("new basket").is_empty());
        assert!(
            row(1)
                .validated()
                .expect("receipted empty basket")
                .is_empty()
        );
        let corruptions: [fn(&mut BasketRow); 12] = [
            |r| r.revision = -1,
            |r| r.rate_window = -1,
            |r| r.mutation_count = -1,
            |r| r.mutation_count = 1001,
            |r| r.lines = None,
            |r| r.lines = Some(serde_json::json!({})),
            |r| r.lines = Some(serde_json::json!([{"sku":"A","quantity":0}])),
            |r| r.last_effect_key = None,
            |r| r.last_effect_key = Some("bad/key".into()),
            |r| r.head_revision = Some(2),
            |r| r.head_expected = Some(2),
            |r| r.head_lines = Some(serde_json::json!([{"sku":"A","quantity":1}])),
        ];
        for corrupt in corruptions {
            let mut r = row(1);
            corrupt(&mut r);
            assert!(matches!(r.validated(), Err(GuestError::InvalidStoredData)));
        }
        let mut r = row(0);
        r.lines = Some(serde_json::json!([{"sku":"A","quantity":1}]));
        assert!(r.validated().is_err());
        let mut r = row(0);
        r.last_effect_key = Some("unexpected".into());
        assert!(r.validated().is_err());
        let mut r = row(0);
        r.head_revision = Some(1);
        assert!(r.validated().is_err());
        for (expected_revision, revision) in [(-1, 0), (i32::MAX, 0), (0, 2)] {
            assert!(matches!(
                Effect {
                    expected_revision,
                    revision,
                    lines: Some(serde_json::json!([]))
                }
                .validated(),
                Err(GuestError::InvalidStoredData)
            ));
        }
        assert!(
            Effect {
                expected_revision: 0,
                revision: 1,
                lines: Some(serde_json::json!([]))
            }
            .validated()
            .expect("valid receipt")
            .is_empty()
        );
    }
}
