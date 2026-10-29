use super::*;
use sha2::{Digest, Sha256};

/// Only exact current SKU/locale/publication derivatives can cross this boundary.
/// No staff token, private asset ID, original or caller-supplied storage path is accepted.
pub async fn published_image(
    pool: &PgPool,
    clock: &dyn Clock,
    locale: Locale,
    sku: &str,
    revision: i32,
    edge: i32,
) -> Result<Option<PrivateRendition>, PublishedError> {
    if revision <= 0 || ![320, 640, 960].contains(&edge) {
        return Err(PublishedError::InvalidRequest);
    }
    let Some(product) = published_product(pool, clock, locale, sku).await? else {
        return Ok(None);
    };
    let Some(expected) = product.image.renditions.iter().find(|r| r.edge == edge) else {
        return Ok(None);
    };
    if product.publication_revision != revision {
        return Ok(None);
    }
    let mut tx = begin(pool).await?;
    let row = image_ingestion::load_output(&mut tx, &product.image.asset_id, edge).await?;
    tx.rollback().await?;
    let Some(row) = row else {
        return Err(PublishedError::InvalidStoredData);
    };
    let bytes = row.validate().await?;
    check_identity(&bytes, expected)?;
    // Withdrawal, replacement, rights expiry or source changes during decoding win.
    let Some(fresh) = published_product(pool, clock, locale, sku).await? else {
        return Ok(None);
    };
    Ok(finish(
        bytes,
        fresh.publication_revision,
        &fresh.image,
        revision,
        &product.image,
    ))
}

// A different activation or selected rendition set cannot reuse the prior bytes.
fn finish(
    bytes: PrivateRendition,
    current_revision: i32,
    current_image: &PublishedImage,
    requested_revision: i32,
    selected_image: &PublishedImage,
) -> Option<PrivateRendition> {
    if current_revision != requested_revision || current_image != selected_image {
        return None;
    }
    Some(bytes)
}

fn check_identity(
    bytes: &PrivateRendition,
    expected: &PublishedRendition,
) -> Result<(), PublishedError> {
    if bytes.width != expected.width
        || bytes.height != expected.height
        || Sha256::digest(&bytes.bytes)[..] != expected.sha256
    {
        return Err(PublishedError::InvalidStoredData);
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../tests/support/published_image.rs"]
mod tests;
