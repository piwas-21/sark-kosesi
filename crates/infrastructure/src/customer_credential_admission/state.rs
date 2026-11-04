use super::{AdmissionError, AdmissionLimits, MAX_QUOTE_UNIX_SECONDS, WINDOW};

#[derive(sqlx::FromRow)]
pub(super) struct Scope {
    window_start: Option<i64>,
    last_seen_at: Option<i64>,
    key_version: Option<i32>,
    key_check: Option<Vec<u8>>,
    global_limit: Option<i32>,
    target_limit: Option<i32>,
    pub(super) admitted: i32,
}

pub(super) struct Previous {
    pub(super) window_start: i64,
    pub(super) last_seen_at: i64,
    pub(super) key_version: i32,
    pub(super) key_check: Vec<u8>,
    pub(super) limits: AdmissionLimits,
}

impl Scope {
    pub(super) fn validate(&self) -> Result<Option<Previous>, AdmissionError> {
        match (
            &self.window_start,
            &self.last_seen_at,
            &self.key_version,
            &self.key_check,
            &self.global_limit,
            &self.target_limit,
        ) {
            (None, None, None, None, None, None) if self.admitted == 0 => Ok(None),
            (Some(window), Some(last), Some(version), Some(check), Some(global), Some(target)) => {
                let limits =
                    AdmissionLimits::new(*global, *target).ok_or(AdmissionError::Unavailable)?;
                if !(0..=MAX_QUOTE_UNIX_SECONDS).contains(window)
                    || window % WINDOW != 0
                    || *last < *window
                    || *last > MAX_QUOTE_UNIX_SECONDS
                    || *last - *window >= WINDOW
                    || *version < 1
                    || check.len() != 32
                    || !(0..=*global).contains(&self.admitted)
                {
                    return Err(AdmissionError::Unavailable);
                }
                Ok(Some(Previous {
                    window_start: *window,
                    last_seen_at: *last,
                    key_version: *version,
                    key_check: check.clone(),
                    limits,
                }))
            }
            _ => Err(AdmissionError::Unavailable),
        }
    }
}
