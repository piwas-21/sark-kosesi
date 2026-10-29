use super::*;
use commerce_domain::{
    ComponentKind,
    catalogue_authoring::{BomLineDraft, Fill, matches_fill},
};

async fn components(
    tx: &mut Transaction<'_, Postgres>,
    lines: &[BomLineDraft],
    fill: Fill,
) -> Result<(), DefinitionError> {
    let codes: Vec<&str> = lines
        .iter()
        .map(|line| line.component_code.as_str())
        .collect();
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM physical_components WHERE code=ANY($1) AND NOT fixture ORDER BY code",
    )
    .bind(codes)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != lines.len() {
        return Err(DefinitionError::NotFound);
    }
    let kinds = rows
        .into_iter()
        .map(|kind| {
            serde_json::from_value::<ComponentKind>(serde_json::Value::String(kind))
                .map_err(|_| DefinitionError::InvalidStoredData)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !matches_fill(fill, &kinds) {
        return Err(DefinitionError::InvalidRequest);
    }
    Ok(())
}
async fn store_bom(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    revision: i32,
    lines: &[BomLineDraft],
    key: &str,
) -> Result<(), DefinitionError> {
    sqlx::query("INSERT INTO catalogue_boms(sku,revision,creation_effect_key) VALUES($1,$2,$3)")
        .bind(sku)
        .bind(revision)
        .bind(key)
        .execute(&mut **tx)
        .await?;
    for line in lines {
        sqlx::query("INSERT INTO catalogue_bom_lines(sku,revision,component_code,quantity) VALUES($1,$2,$3,$4)").bind(sku).bind(revision).bind(&line.component_code).bind(i32::try_from(line.quantity).map_err(|_|DefinitionError::InvalidRequest)?).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE catalogue_boms SET sealed=true WHERE sku=$1 AND revision=$2")
        .bind(sku)
        .bind(revision)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub(super) async fn command(
    tx: &mut Transaction<'_, Postgres>,
    command: &CatalogueCommand,
    key: &str,
) -> Result<(), DefinitionError> {
    match command {
        CatalogueCommand::Family { code } => {
            let inserted:Option<(String,)>=sqlx::query_as("INSERT INTO product_families(code,fixture,creation_effect_key) VALUES($1,false,$2) ON CONFLICT DO NOTHING RETURNING code").bind(code).bind(key).fetch_optional(&mut **tx).await?;
            inserted.ok_or(DefinitionError::Conflict)?;
        }
        CatalogueCommand::Component {
            code,
            component_kind,
        } => {
            let inserted:Option<(String,)>=sqlx::query_as("INSERT INTO physical_components(code,kind,fixture,creation_effect_key) VALUES($1,$2,false,$3) ON CONFLICT DO NOTHING RETURNING code").bind(code).bind(component_kind.code()).bind(key).fetch_optional(&mut **tx).await?;
            inserted.ok_or(DefinitionError::Conflict)?;
        }
        CatalogueCommand::Variant {
            sku,
            family_code,
            size_code,
            textile_code,
            fill,
            lines,
        } => {
            let family: Option<(String,)> =
                sqlx::query_as("SELECT code FROM product_families WHERE code=$1 AND NOT fixture")
                    .bind(family_code)
                    .fetch_optional(&mut **tx)
                    .await?;
            family.ok_or(DefinitionError::NotFound)?;
            components(tx, lines, *fill).await?;
            let inserted:Option<(String,)>=sqlx::query_as("INSERT INTO catalogue_items(sku,slug,minor_units,currency,fill,fixture,creation_effect_key) VALUES($1,NULL,NULL,'EUR',$2,false,$3) ON CONFLICT DO NOTHING RETURNING sku")
                .bind(sku).bind(fill.code()).bind(key).fetch_optional(&mut **tx).await?;
            inserted.ok_or(DefinitionError::Conflict)?;
            sqlx::query("INSERT INTO catalogue_variants(sku,family_code,size_code,textile_code) VALUES($1,$2,$3,$4)").bind(sku).bind(family_code).bind(size_code).bind(textile_code).execute(&mut **tx).await?;
            store_bom(tx, sku, 1, lines, key).await?;
        }
        CatalogueCommand::Bom {
            sku,
            expected_revision,
            lines,
        } => {
            let fill:Option<String>=sqlx::query_scalar("UPDATE catalogue_variants v SET stock_policy=v.stock_policy FROM catalogue_items i WHERE v.sku=$1 AND i.sku=v.sku AND NOT i.fixture RETURNING i.fill").bind(sku).fetch_optional(&mut **tx).await?;
            let fill: Fill = serde_json::from_value(serde_json::Value::String(
                fill.ok_or(DefinitionError::NotFound)?,
            ))
            .map_err(|_| DefinitionError::InvalidStoredData)?;
            let (current,): (i32,) =
                sqlx::query_as("SELECT coalesce(max(revision),0) FROM catalogue_boms WHERE sku=$1")
                    .bind(sku)
                    .fetch_one(&mut **tx)
                    .await?;
            if current != *expected_revision {
                return Err(DefinitionError::Conflict);
            }
            components(tx, lines, fill).await?;
            store_bom(
                tx,
                sku,
                current
                    .checked_add(1)
                    .ok_or(DefinitionError::InvalidRequest)?,
                lines,
                key,
            )
            .await?;
        }
    }
    Ok(())
}
