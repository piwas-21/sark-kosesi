use super::*;
/// Started means a request may have reached the provider, even if no reply was saved.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DispatchState {
    Prepared,
    Started {
        started_at: i64,
    },
    Known {
        started_at: i64,
        provider_id: String,
    },
}
pub enum DispatchPlan {
    Start {
        next: DispatchState,
        idempotency_key: String,
    },
    Reconcile {
        provider_id: Option<String>,
    },
}
impl DispatchState {
    pub fn validate(&self, attempt: &AttemptSnapshot) -> Result<(), PaymentError> {
        attempt.validate()?;
        match self {
            Self::Prepared => Ok(()),
            Self::Started { started_at } | Self::Known { started_at, .. } => {
                if *started_at < attempt.binding.created_at
                    || *started_at >= attempt.binding.dispatch_before
                {
                    return Err(PaymentError::InvalidSnapshot);
                }
                if let Self::Known { provider_id, .. } = self
                    && !opaque(provider_id, "tr_")
                {
                    return Err(PaymentError::InvalidSnapshot);
                }
                Ok(())
            }
        }
    }
}
/// Pure plan only. The store must atomically persist Start exactly once before I/O.
/// A started/unknown attempt never authorizes another POST, even within Mollie's cache.
pub fn plan_dispatch(
    attempt: &AttemptSnapshot,
    state: &DispatchState,
    at: i64,
    configuration: &CurrentConfiguration,
) -> Result<DispatchPlan, PaymentError> {
    state.validate(attempt)?;
    if at < attempt.binding.created_at || at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS {
        return Err(PaymentError::InvalidClock);
    }
    match state {
        DispatchState::Prepared => {
            if at >= attempt.binding.dispatch_before {
                return Err(PaymentError::Expired);
            }
            if configuration.profile != attempt.binding.profile
                || configuration.sha256 != attempt.binding.configuration_sha256
            {
                return Err(PaymentError::ConfigurationChanged);
            }
            Ok(DispatchPlan::Start {
                next: DispatchState::Started { started_at: at },
                idempotency_key: attempt.idempotency_key(),
            })
        }
        DispatchState::Started { started_at } | DispatchState::Known { started_at, .. } => {
            if at < *started_at {
                return Err(PaymentError::InvalidClock);
            }
            Ok(DispatchPlan::Reconcile {
                provider_id: match state {
                    DispatchState::Known { provider_id, .. } => Some(provider_id.clone()),
                    _ => None,
                },
            })
        }
    }
}
/// Bind a matched provider identity without inferring payment or fulfilment state.
pub fn bind_provider(
    attempt: &AttemptSnapshot,
    state: &DispatchState,
    provider_id: &str,
) -> Result<DispatchState, PaymentError> {
    state.validate(attempt)?;
    if !opaque(provider_id, "tr_") {
        return Err(PaymentError::ProviderMismatch);
    }
    match state {
        DispatchState::Prepared => Err(PaymentError::NotDispatched),
        DispatchState::Started { started_at } => Ok(DispatchState::Known {
            started_at: *started_at,
            provider_id: provider_id.into(),
        }),
        DispatchState::Known {
            provider_id: existing,
            ..
        } => {
            if existing == provider_id {
                Ok(state.clone())
            } else {
                Err(PaymentError::IdentityConflict)
            }
        }
    }
}
