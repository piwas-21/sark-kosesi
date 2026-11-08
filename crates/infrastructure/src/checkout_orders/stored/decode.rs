use super::*;
fn objects(value: &serde_json::Value, keys: &[&str]) -> bool {
    value.is_object()
        && keys
            .iter()
            .all(|key| value.get(*key).is_some_and(serde_json::Value::is_object))
}
impl Row {
    pub(super) fn decode(self) -> Result<(OrderSnapshot, OrderIntent), OrderStoreError> {
        let intent = self.intent.ok_or(OrderStoreError::InvalidStoredData)?;
        let value = self.snapshot.ok_or(OrderStoreError::InvalidStoredData)?;
        if !objects(&intent, &["buyer", "shipping_address", "billing_address"])
            || !objects(
                &value,
                &[
                    "identity",
                    "quote",
                    "buyer",
                    "shipping_address",
                    "billing_address",
                    "seller",
                    "market",
                ],
            )
            || !objects(&value["seller"], &["address", "tax_registration"])
            || !objects(&value["market"], &["destination", "shipping"])
            || !objects(&value["market"]["shipping"], &["gross"])
            || !value["market"]["products"].as_array().is_some_and(|rows| {
                rows.len() <= 20 && rows.iter().all(serde_json::Value::is_object)
            })
            || !value["products"].as_array().is_some_and(|rows| {
                rows.len() <= 20 && rows.iter().all(|row| objects(row, &["footprint"]))
            })
        {
            return Err(OrderStoreError::InvalidStoredData);
        }
        let quote_value = &value["quote"];
        if value["schema_version"].as_u64() == Some(2) {
            if !objects(quote_value, &["original", "promotion", "shipping", "total"])
                || !quote_value["lines"].as_array().is_some_and(|rows| {
                    rows.len() <= 20 && rows.iter().all(|row| objects(row, &["amounts"]))
                })
            {
                return Err(OrderStoreError::InvalidStoredData);
            }
            crate::checkout_quotes::decode_snapshot(quote_value["original"].clone())?;
        } else {
            crate::checkout_quotes::decode_snapshot(quote_value.clone())?;
        }
        if self.original_quote.as_ref() != Some(quote_value) {
            return Err(OrderStoreError::InvalidStoredData);
        }
        let snapshot: OrderSnapshot =
            serde_json::from_value(value).map_err(|_| OrderStoreError::InvalidStoredData)?;
        let intent: OrderIntent =
            serde_json::from_value(intent).map_err(|_| OrderStoreError::InvalidStoredData)?;
        snapshot
            .validate()
            .map_err(|_| OrderStoreError::InvalidStoredData)?;
        intent
            .validate()
            .map_err(|_| OrderStoreError::InvalidStoredData)?;
        let id = &snapshot.identity;
        let captured_header = (
            &id.id,
            &id.basket_id,
            &id.quote_id,
            &id.stock_group_id,
            id.created_at,
            id.hold_expires_at,
            snapshot.quote.binding.basket_revision,
        );
        let stored_header = (
            &self.id,
            &self.basket_id,
            &self.quote_id,
            &self.stock_group_id,
            self.created_at,
            self.hold_expires_at,
            self.basket_revision,
        );
        if captured_header != stored_header
            || !valid_command_id(&self.effect_key)
            || intent.quote_id != self.quote_id
            || intent.expected_basket_revision != self.basket_revision
            || snapshot.buyer != intent.buyer
            || snapshot.shipping_address != intent.shipping_address
            || snapshot.billing_address != intent.billing_address
            || snapshot.seller.terms_version != intent.accepted_terms_version
            || self.original_market
                != Some(
                    serde_json::to_value(&snapshot.market)
                        .map_err(|_| OrderStoreError::InvalidStoredData)?,
                )
        {
            return Err(OrderStoreError::InvalidStoredData);
        }
        Ok((snapshot, intent))
    }
}
