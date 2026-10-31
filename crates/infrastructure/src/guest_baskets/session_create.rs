use super::*;
#[derive(sqlx::FromRow)]
struct Quota {
    rate_window: i64,
    issued: i32,
}
impl GuestSessionStore {
    pub(super) async fn create_record(&self, record: &mut Record) -> Result<(), GuestError> {
        let identity = GuestIdentity::from_record(record)?;
        let at = now(self.clock.as_ref())?;
        identity.check(at)?;
        // Always assign a fresh checked OS identifier. Never retain an unknown cookie ID.
        record.id = Id(i128::from_le_bytes(self.identifiers.next()?));
        let mut tx = begin(&self.pool).await?;
        let quota = sqlx::query_as::<_, Quota>(
            "SELECT rate_window,issued FROM guest_issue_quota WHERE singleton FOR UPDATE",
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(GuestError::InvalidStoredData)?;
        let admitted = now(self.clock.as_ref())?;
        if admitted < at {
            return Err(GuestError::ClockUnavailable);
        }
        identity.check(admitted)?;
        if quota.rate_window < 0 || !(0..=1000).contains(&quota.issued) {
            return Err(GuestError::InvalidStoredData);
        }
        let window = admitted / 60;
        if window < quota.rate_window {
            return Err(GuestError::ClockUnavailable);
        }
        let issued = if window == quota.rate_window {
            quota.issued
        } else {
            0
        };
        if issued >= i32::from(self.limits.issue_per_minute) {
            return Err(GuestError::RateLimited);
        }
        let issued = issued.checked_add(1).ok_or(GuestError::InvalidStoredData)?;
        sqlx::query("UPDATE guest_issue_quota SET rate_window=$1,issued=$2 WHERE singleton")
            .bind(window)
            .bind(issued)
            .execute(&mut *tx)
            .await?;
        // A principal-ID collision fails the whole transaction, never links another basket.
        sqlx::query("INSERT INTO guest_baskets(id,created_at) VALUES($1,$2)")
            .bind(&identity.basket_id)
            .bind(identity.created_at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO guest_sessions(token_hash,basket_id,csrf_token,created_at,expires_at) VALUES($1,$2,$3,$4,$5)").bind(digest(record.id).as_slice()).bind(&identity.basket_id).bind(&identity.csrf_token).bind(identity.created_at).bind(identity.expires_at).execute(&mut *tx).await?;
        let finished = now(self.clock.as_ref())?;
        if finished < admitted {
            return Err(GuestError::ClockUnavailable);
        }
        identity.check(finished)?;
        tx.commit().await?;
        Ok(())
    }
}
