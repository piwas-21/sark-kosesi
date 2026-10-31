//! One bounded closed snapshot decoder shared by quote and immutable order readers.
use super::*;
pub(crate) fn decode_snapshot(value: serde_json::Value) -> Result<QuoteSnapshot, QuoteStoreError> {
    let object = value
        .as_object()
        .ok_or(QuoteStoreError::InvalidStoredData)?;
    if !["binding", "shipping", "total"]
        .iter()
        .all(|key| object.get(*key).is_some_and(serde_json::Value::is_object))
        || !object
            .get("lines")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|lines| {
                lines.len() <= 20
                    && lines.iter().all(|line| {
                        line.is_object()
                            && line
                                .get("amounts")
                                .is_some_and(serde_json::Value::is_object)
                    })
            })
    {
        return Err(QuoteStoreError::InvalidStoredData);
    }
    let snapshot: QuoteSnapshot =
        serde_json::from_value(value).map_err(|_| QuoteStoreError::InvalidStoredData)?;
    snapshot.totals()?;
    Ok(snapshot)
}
