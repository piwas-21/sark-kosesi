use super::*;
use sqlx::{Postgres, Transaction};
#[derive(sqlx::FromRow)]
struct Stored {
    asset_id: String,
    sha256: Vec<u8>,
    actor: String,
    reason: String,
}
pub(super) async fn persist(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: IngestImageRequest<'_>,
    decoded: process::Decoded,
) -> Result<ImageReceipt, ImageError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("image-ingestion:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let claim: Option<(String,)> = sqlx::query_as(
        "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=$1 FOR UPDATE",
    )
    .bind(request.asset_id)
    .fetch_optional(&mut *access.tx)
    .await?;
    if claim.is_none() {
        return Err(ImageError::InvalidStoredData);
    }
    let metadata = media::asset(&mut access.tx, request.asset_id)
        .await?
        .ok_or(ImageError::NotFound)?;
    if metadata != decoded.registered {
        return Err(ImageError::MetadataMismatch);
    }
    let prior = sqlx::query_as::<_, Stored>(
        "SELECT asset_id,sha256,actor,reason FROM catalogue_image_originals WHERE effect_key=$1",
    )
    .bind(request.effect_key)
    .fetch_optional(&mut *access.tx)
    .await?;
    if let Some(prior) = prior {
        if prior.asset_id != request.asset_id
            || prior.sha256 != decoded.sha256
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(ImageError::Conflict);
        }
        let result = receipt(&mut access.tx, request.asset_id, true)
            .await?
            .ok_or(ImageError::InvalidStoredData)?;
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(result);
    }
    let source_type = match metadata.image_type {
        commerce_domain::media::ImageType::Jpeg => "jpeg",
        commerce_domain::media::ImageType::Png => "png",
        commerce_domain::media::ImageType::Webp => "webp",
    };
    let inserted:Option<(String,)>=sqlx::query_as("INSERT INTO catalogue_image_originals(asset_id,effect_key,sha256,bytes,image_type,source_width_px,source_height_px,display_width_px,display_height_px,actor,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT DO NOTHING RETURNING asset_id")
        .bind(request.asset_id).bind(request.effect_key).bind(decoded.sha256.as_slice()).bind(decoded.original).bind(source_type)
        .bind(i32::try_from(metadata.width_px).map_err(|_|ImageError::InvalidStoredData)?).bind(i32::try_from(metadata.height_px).map_err(|_|ImageError::InvalidStoredData)?)
        .bind(decoded.width).bind(decoded.height).bind(&access.actor).bind(request.reason).fetch_optional(&mut *access.tx).await?;
    if inserted.is_none() {
        return Err(ImageError::Conflict);
    }
    for rendition in decoded.renditions {
        sqlx::query("INSERT INTO catalogue_image_renditions(asset_id,edge,width_px,height_px,image_type,sha256,bytes) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(request.asset_id).bind(rendition.edge).bind(rendition.width).bind(rendition.height).bind(rendition.image_type)
            .bind(rendition.sha256.as_slice()).bind(rendition.bytes).execute(&mut *access.tx).await?;
    }
    let result = receipt(&mut access.tx, request.asset_id, false)
        .await?
        .ok_or(ImageError::InvalidStoredData)?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(result)
}
pub(super) async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    asset_id: &str,
    replayed: bool,
) -> Result<Option<ImageReceipt>, ImageError> {
    let row:Option<(i32,i32)>=sqlx::query_as("SELECT display_width_px,display_height_px FROM catalogue_image_originals WHERE asset_id=$1 AND recipe='image-v1'").bind(asset_id).fetch_optional(&mut **tx).await?;
    let Some((display_width, display_height)) = row else {
        return Ok(None);
    };
    let rows:Vec<(i32,i32,i32,String,Vec<u8>,i32)>=sqlx::query_as("SELECT edge,width_px,height_px,image_type,sha256,octet_length(bytes) FROM catalogue_image_renditions WHERE asset_id=$1 AND recipe='image-v1' ORDER BY edge")
        .bind(asset_id).fetch_all(&mut **tx).await?;
    assemble(asset_id, display_width, display_height, rows, replayed).map(Some)
}
type RawRendition = (i32, i32, i32, String, Vec<u8>, i32);
fn assemble(
    asset_id: &str,
    display_width: i32,
    display_height: i32,
    rows: Vec<RawRendition>,
    replayed: bool,
) -> Result<ImageReceipt, ImageError> {
    let longest = display_width.max(display_height);
    let mut expected = vec![320];
    if longest > 320 {
        expected.push(640);
    }
    if longest > 640 {
        expected.push(960);
    }
    if display_width > 16384
        || display_height > 16384
        || i64::from(display_width) * i64::from(display_height) > 67108864
        || display_width <= 0
        || display_height <= 0
        || rows.iter().map(|row| row.0).collect::<Vec<_>>() != expected
        || rows.iter().any(|(edge, w, h, kind, digest, length)| {
            *w <= 0
                || *h <= 0
                || (*w).max(*h) != (*edge).min(longest)
                || !["jpeg", "png"].contains(&kind.as_str())
                || digest.len() != 32
                || *length <= 0
                || *length > 256000
        })
    {
        return Err(ImageError::InvalidStoredData);
    }
    let renditions = rows
        .into_iter()
        .map(
            |(edge, width, height, image_type, sha256, byte_length)| RenditionMetadata {
                edge,
                width,
                height,
                image_type,
                sha256,
                byte_length,
            },
        )
        .collect();
    Ok(ImageReceipt {
        asset_id: asset_id.into(),
        recipe: RECIPE,
        display_width,
        display_height,
        renditions,
        replayed,
    })
}

#[cfg(test)]
#[path = "../../tests/support/image_store.rs"]
mod tests;
