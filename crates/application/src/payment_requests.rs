//! Frozen test-payment requests. No environment access, transport or dispatch authority.
use commerce_domain::{
    Currency,
    payment_attempts::{ATTEMPT_SCHEMA_VERSION, AttemptBinding, AttemptSnapshot},
};
use serde_json::{Map, Value};
mod config;
pub use config::{Configuration, ConfigurationSnapshot, PaymentMethod};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidConfiguration,
    InvalidBinding,
    InvalidStoredRequest,
}
pub struct FrozenAttempt {
    snapshot: AttemptSnapshot,
    body: String,
}
impl FrozenAttempt {
    pub fn capture(
        binding: AttemptBinding,
        configuration: &Configuration,
    ) -> Result<Self, RequestError> {
        binding
            .validate()
            .map_err(|_| RequestError::InvalidBinding)?;
        let current = configuration.identity();
        if binding.profile != current.profile || binding.configuration_sha256 != current.sha256 {
            return Err(RequestError::InvalidConfiguration);
        }
        let mut redirect = configuration.return_url();
        redirect
            .query_pairs_mut()
            .append_pair("order_id", &binding.order_id)
            .append_pair("attempt_id", &binding.id);
        let currency = match binding.currency {
            Currency::EUR => "EUR",
        };
        // Only bounded validated strings/integers enter; no contact/address fields or raw keys.
        let body = object([
            (
                "amount",
                object([
                    ("currency", Value::String(currency.into())),
                    (
                        "value",
                        Value::String(format!(
                            "{}.{:02}",
                            binding.amount_minor / 100,
                            binding.amount_minor % 100
                        )),
                    ),
                ]),
            ),
            (
                "description",
                Value::String(format!("Piwas order {}", binding.order_id)),
            ),
            ("redirectUrl", Value::String(redirect.into())),
            (
                "webhookUrl",
                Value::String(configuration.webhook_url().into()),
            ),
            (
                "method",
                Value::String(configuration.method().code().into()),
            ),
            ("sequenceType", Value::String("oneoff".into())),
            (
                "metadata",
                object([
                    ("schema_version", Value::from(ATTEMPT_SCHEMA_VERSION)),
                    ("order_id", Value::String(binding.order_id.clone())),
                    ("attempt_id", Value::String(binding.id.clone())),
                ]),
            ),
        ])
        .to_string();
        let snapshot = AttemptSnapshot {
            schema_version: ATTEMPT_SCHEMA_VERSION,
            binding,
            request_sha256: config::digest(body.as_bytes()),
        };
        Ok(Self { snapshot, body })
    }
    /// Stored bytes must exactly reproduce the versioned canonical request under its captured policy.
    pub fn restore(
        snapshot: &AttemptSnapshot,
        body: &str,
        configuration: &Configuration,
    ) -> Result<Self, RequestError> {
        if body.len() > 8192 || snapshot.validate().is_err() {
            return Err(RequestError::InvalidStoredRequest);
        }
        let frozen = Self::capture(snapshot.binding.clone(), configuration)?;
        if frozen.snapshot != *snapshot || frozen.body != body {
            return Err(RequestError::InvalidStoredRequest);
        }
        Ok(frozen)
    }
    pub fn snapshot(&self) -> &AttemptSnapshot {
        &self.snapshot
    }
    pub fn body(&self) -> &str {
        &self.body
    }
}
fn object<const N: usize>(mut entries: [(&str, Value); N]) -> Value {
    // Keep canonical ordering even if another crate enables serde_json preserve_order.
    entries.sort_by(|a, b| a.0.cmp(b.0));
    Value::Object(Map::from_iter(
        entries.into_iter().map(|(key, value)| (key.into(), value)),
    ))
}
