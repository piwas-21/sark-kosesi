use super::*;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GuestIdentity {
    pub basket_id: String,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
}
impl GuestIdentity {
    pub fn lifetime(&self) -> GuestLifetime {
        GuestLifetime {
            created_at: self.created_at,
            expires_at: self.expires_at,
        }
    }
    pub fn validate(&self) -> Result<(), GuestError> {
        if !valid_hex(&self.basket_id, 32)
            || !valid_hex(&self.csrf_token, 64)
            || !self.lifetime().valid()
        {
            return Err(GuestError::InvalidStoredData);
        }
        Ok(())
    }
    pub fn check(&self, at: i64) -> Result<(), GuestError> {
        self.validate()?;
        self.lifetime().check(at).map_err(|_| GuestError::Denied)
    }
    pub fn from_record(record: &Record) -> Result<Self, GuestError> {
        if record.data.len() != 1 {
            return Err(GuestError::InvalidStoredData);
        }
        let value = record
            .data
            .get("guest")
            .ok_or(GuestError::InvalidStoredData)?;
        let identity: Self =
            serde_json::from_value(value.clone()).map_err(|_| GuestError::InvalidStoredData)?;
        identity.validate()?;
        if record.expiry_date.unix_timestamp() != identity.expires_at {
            return Err(GuestError::InvalidStoredData);
        }
        Ok(identity)
    }
    pub fn expiry(&self) -> Result<OffsetDateTime, GuestError> {
        OffsetDateTime::from_unix_timestamp(self.expires_at)
            .map_err(|_| GuestError::InvalidStoredData)
    }
    pub fn record(&self, id: Id) -> Result<Record, GuestError> {
        self.validate()?;
        let data = std::collections::HashMap::from([(
            "guest".into(),
            serde_json::to_value(self).map_err(|_| GuestError::InvalidStoredData)?,
        )]);
        Ok(Record {
            id,
            data,
            expiry_date: self.expiry()?,
        })
    }
}
#[derive(sqlx::FromRow)]
pub(super) struct SessionRow {
    pub basket_id: String,
    pub csrf_token: String,
    pub created_at: i64,
    pub expires_at: i64,
}
impl SessionRow {
    pub fn identity(self) -> GuestIdentity {
        GuestIdentity {
            basket_id: self.basket_id,
            csrf_token: self.csrf_token,
            created_at: self.created_at,
            expires_at: self.expires_at,
        }
    }
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8],
    exclusive: bool,
) -> Result<Option<GuestIdentity>, GuestError> {
    let query = if exclusive {
        "SELECT left(basket_id,33) AS basket_id,left(csrf_token,65) AS csrf_token,created_at,expires_at FROM guest_sessions WHERE token_hash=$1 FOR UPDATE"
    } else {
        "SELECT left(basket_id,33) AS basket_id,left(csrf_token,65) AS csrf_token,created_at,expires_at FROM guest_sessions WHERE token_hash=$1 FOR SHARE"
    };
    let row = sqlx::query_as::<_, SessionRow>(query)
        .bind(digest)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.map(SessionRow::identity))
}
