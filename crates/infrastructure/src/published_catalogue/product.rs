use super::*;
pub(super) fn lines(sources: &publication::Sources) -> Result<&[BomLineDraft], PublishedError> {
    match &sources.facts.bom.command {
        CatalogueCommand::Variant { lines, .. } | CatalogueCommand::Bom { lines, .. } => Ok(lines),
        _ => Err(PublishedError::InvalidStoredData),
    }
}
pub(super) fn assemble(
    sku: String,
    locale: Locale,
    revision: i32,
    selection: ApprovedSelection,
    sources: publication::Sources,
    in_stock: bool,
) -> Result<PublishedProduct, PublishedError> {
    let (size_code, textile_code, fill) = match &sources.facts.identity.command {
        CatalogueCommand::Variant {
            size_code,
            textile_code,
            fill,
            ..
        } => (
            size_code.clone(),
            textile_code.clone(),
            fill.code().to_string(),
        ),
        _ => return Err(PublishedError::InvalidStoredData),
    };
    let components = lines(&sources)?
        .iter()
        .map(|line| {
            let spec = sources
                .facts
                .specification
                .draft
                .components
                .iter()
                .find(|spec| spec.component_code == line.component_code)
                .ok_or(PublishedError::InvalidStoredData)?;
            let material = match &spec.material {
                MaterialSpecification::Cover { fibres, .. } => PublishedMaterial::Cover {
                    fibres: fibres.clone(),
                },
                MaterialSpecification::Foam {
                    density_g_per_m3, ..
                } => PublishedMaterial::Foam {
                    density_g_per_m3: *density_g_per_m3,
                },
            };
            Ok(PublishedComponent {
                code: line.component_code.clone(),
                quantity: line.quantity,
                dimensions: spec.dimensions.clone(),
                material,
            })
        })
        .collect::<Result<Vec<_>, PublishedError>>()?;
    let parcels = sources
        .facts
        .specification
        .draft
        .parcels
        .into_iter()
        .map(|p| PublishedParcel {
            dimensions: p.dimensions,
            gross_weight_g: p.gross_weight_g,
        })
        .collect();
    let renditions = sources
        .image
        .image
        .renditions
        .into_iter()
        .map(|r| PublishedRendition {
            edge: r.edge,
            width: r.width,
            height: r.height,
            sha256: r.sha256,
        })
        .collect();
    Ok(PublishedProduct {
        sku,
        locale,
        publication_revision: revision,
        family_code: sources.family_code,
        family_slug: selection.family_slug,
        size_code,
        textile_code,
        fill,
        price: sources.facts.price,
        copy: sources.copy.draft,
        footprint: sources.facts.specification.draft.footprint,
        components,
        parcels,
        image: PublishedImage {
            asset_id: selection.image.asset_id,
            renditions,
        },
        in_stock,
    })
}
