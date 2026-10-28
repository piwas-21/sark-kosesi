//! Only sealed revisions may become reservation inputs. Drafts remain invisible.

use commerce_domain::{BillOfMaterials, BomComponent, ComponentKind};
use sqlx::PgPool;

#[derive(sqlx::FromRow)]
struct ComponentRow {
    code: String,
    kind: String,
    quantity: i32,
}

pub async fn bill_of_materials(
    pool: &PgPool,
    sku: &str,
    revision: i32,
) -> Result<Option<BillOfMaterials>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ComponentRow>(
        "SELECT c.code, c.kind, l.quantity FROM catalogue_boms b
         JOIN catalogue_bom_lines l USING (sku, revision)
         JOIN physical_components c ON c.code = l.component_code
         WHERE b.sku = $1 AND b.revision = $2 AND b.sealed ORDER BY c.code",
    )
    .bind(sku)
    .bind(revision)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let components = rows
        .into_iter()
        .map(|row| {
            let kind = match row.kind.as_str() {
                "cover" => ComponentKind::Cover,
                "foam" => ComponentKind::Foam,
                _ => {
                    return Err(sqlx::Error::Protocol(
                        "Invalid stored component kind".into(),
                    ));
                }
            };
            let quantity = u32::try_from(row.quantity)
                .ok()
                .filter(|value| (1..=10000).contains(value))
                .ok_or_else(|| sqlx::Error::Protocol("Invalid stored BOM quantity".into()))?;
            Ok(BomComponent {
                code: row.code,
                kind,
                quantity,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Some(BillOfMaterials {
        sku: sku.to_owned(),
        revision,
        components,
    }))
}
