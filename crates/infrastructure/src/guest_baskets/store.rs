use super::*;
#[async_trait::async_trait]
impl SessionStore for GuestSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        self.create_record(record).await.map_err(backend)
    }
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        async {
            let identity = GuestIdentity::from_record(record)?;
            let mut tx = begin(&self.pool).await?;
            let stored = identity::load(&mut tx, digest(record.id).as_slice(), false)
                .await?
                .ok_or(GuestError::Denied)?;
            if stored != identity {
                return Err(GuestError::InvalidStoredData);
            }
            stored.check(now(self.clock.as_ref())?)?;
            tx.rollback().await?;
            Ok(())
        }
        .await
        .map_err(backend)
    }
    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        async {
            let mut tx = begin(&self.pool).await?;
            let identity = identity::load(&mut tx, digest(*id).as_slice(), false).await?;
            let result = if let Some(identity) = identity {
                identity.validate()?;
                if identity.lifetime().check(now(self.clock.as_ref())?).is_ok() {
                    Some(identity.record(*id)?)
                } else {
                    None
                }
            } else {
                None
            };
            tx.rollback().await?;
            Ok(result)
        }
        .await
        .map_err(backend)
    }
    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        async {
            let mut tx = begin(&self.pool).await?;
            // DELETE conflicts with every live operation's credential SHARE claim.
            sqlx::query("DELETE FROM guest_sessions WHERE token_hash=$1")
                .bind(digest(*id).as_slice())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(())
        }
        .await
        .map_err(backend)
    }
}
