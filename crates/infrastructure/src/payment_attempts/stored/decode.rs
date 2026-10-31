use super::*;
use commerce_application::payment_requests::ConfigurationSnapshot;
fn object(value: &serde_json::Value, keys: &[&str]) -> bool {
    value.is_object()
        && keys
            .iter()
            .all(|key| value.get(*key).is_some_and(serde_json::Value::is_object))
}
impl Row {
    pub(super) fn decode(self) -> Result<FrozenAttempt, PaymentStoreError> {
        let value = self.snapshot.ok_or(PaymentStoreError::InvalidStoredData)?;
        let policy = self
            .configuration
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        let intent = self.intent.ok_or(PaymentStoreError::InvalidStoredData)?;
        if !object(&value, &["binding"])
            || !object(&value["binding"], &["profile"])
            || !object(&policy, &["profile"])
            || !intent.is_object()
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let snapshot: AttemptSnapshot =
            serde_json::from_value(value).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let policy: ConfigurationSnapshot =
            serde_json::from_value(policy).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let intent: PreparationIntent =
            serde_json::from_value(intent).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        snapshot
            .validate()
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        intent
            .validate()
            .map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let configuration =
            Configuration::capture(policy).map_err(|_| PaymentStoreError::InvalidStoredData)?;
        let b = &snapshot.binding;
        if b.id != self.id
            || b.order_id != self.order_id
            || b.basket_id != self.basket_id
            || b.created_at != self.created_at
            || b.dispatch_before != self.dispatch_before
            || snapshot.idempotency_key() != self.idempotency_key
            || !valid_command_id(&self.effect_key)
            || intent.order_id != self.order_id
            || self.material.as_deref() != Some(configuration.fingerprint_material())
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        let body = self.body.ok_or(PaymentStoreError::InvalidStoredData)?;
        FrozenAttempt::restore(&snapshot, &body, &configuration)
            .map_err(|_| PaymentStoreError::InvalidStoredData)
    }
}
