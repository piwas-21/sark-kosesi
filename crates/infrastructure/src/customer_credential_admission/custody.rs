use sqlx::PgConnection;

use super::{AdmissionError, AdmissionLimits, CredentialPurpose};

pub(super) async fn require_empty_targets(
    connection: &mut PgConnection,
    purpose: CredentialPurpose,
) -> Result<(), AdmissionError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM customer_credential_admission_targets WHERE purpose=$1)",
    )
    .bind(purpose as i16)
    .fetch_one(connection)
    .await
    .map_err(|_| AdmissionError::Unavailable)?;
    if exists {
        return Err(AdmissionError::Unavailable);
    }
    Ok(())
}

pub(super) struct Expected<'a> {
    pub(super) purpose: CredentialPurpose,
    pub(super) window: i64,
    pub(super) last_seen: i64,
    pub(super) key_version: i32,
    pub(super) key_check: &'a [u8],
    pub(super) limits: AdmissionLimits,
    pub(super) total: i32,
    pub(super) digest: &'a [u8],
    pub(super) target: i32,
}

impl Expected<'_> {
    /// Read both records while the purpose lock is held; successful DML alone
    /// does not prove that a trigger retained the intended values.
    pub(super) async fn require_stored(
        &self,
        connection: &mut PgConnection,
    ) -> Result<(), AdmissionError> {
        let stored: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM customer_credential_admission_scopes s \
             JOIN customer_credential_admission_targets t ON t.purpose=s.purpose \
             WHERE s.purpose=$1 AND s.window_start=$2 AND s.last_seen_at=$3 \
             AND s.key_version=$4 AND s.key_check=$5 AND s.global_limit=$6 \
             AND s.target_limit=$7 AND s.admitted=$8 AND t.target_digest=$9 AND t.admitted=$10)",
        )
        .bind(self.purpose as i16)
        .bind(self.window)
        .bind(self.last_seen)
        .bind(self.key_version)
        .bind(self.key_check)
        .bind(self.limits.global)
        .bind(self.limits.target)
        .bind(self.total)
        .bind(self.digest)
        .bind(self.target)
        .fetch_one(connection)
        .await
        .map_err(|_| AdmissionError::Unavailable)?;
        if !stored {
            return Err(AdmissionError::Unavailable);
        }
        Ok(())
    }
}
