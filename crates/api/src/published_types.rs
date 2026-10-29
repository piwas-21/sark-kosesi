//! Explicit customer-facing whitelist; authoring and review bundles are never serialized.
use commerce_infrastructure::published_catalogue as source;
use serde::Serialize;
use utoipa::ToSchema;
#[derive(Debug, Serialize, ToSchema)]
pub struct PublishedCatalogue {
    pub items: Vec<PublishedProduct>,
    pub locale: String,
    pub next_after: Option<String>,
    pub production_enabled: bool,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct PublishedProduct {
    pub sku: String,
    pub locale: String,
    pub publication_revision: i32,
    pub family_code: String,
    pub family_slug: String,
    pub size_code: String,
    pub textile_code: String,
    pub fill: String,
    pub minor_units: i64,
    pub currency: String,
    pub price_decimal: String,
    pub copy: ProductCopy,
    pub footprint: Measurements,
    pub components: Vec<Component>,
    pub parcels: Vec<Parcel>,
    pub images: Vec<ProductImage>,
    pub availability: Availability,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct ProductCopy {
    pub title: String,
    pub description: String,
    pub materials: String,
    pub care: String,
    pub safety: String,
    pub assembly: String,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct Measurements {
    pub width_mm: u32,
    pub depth_mm: u32,
    pub height_mm: u32,
    pub tolerance_mm: u32,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct Component {
    pub code: String,
    pub quantity: u32,
    pub dimensions: Measurements,
    pub material: Material,
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Material {
    Cover { fibres: Vec<Fibre> },
    Foam { density_g_per_m3: u32 },
}
#[derive(Debug, Serialize, ToSchema)]
pub struct Fibre {
    pub name: String,
    pub basis_points: u16,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct Parcel {
    pub dimensions: Measurements,
    pub gross_weight_g: u32,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct ProductImage {
    pub url: String,
    pub edge: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    InStock,
    OutOfStock,
}
fn measurements(v: commerce_domain::specifications::DimensionsMm) -> Measurements {
    Measurements {
        width_mm: v.width_mm,
        depth_mm: v.depth_mm,
        height_mm: v.height_mm,
        tolerance_mm: v.tolerance_mm,
    }
}
impl From<source::PublishedProduct> for PublishedProduct {
    fn from(p: source::PublishedProduct) -> Self {
        let images = p
            .image
            .renditions
            .into_iter()
            .map(|r| ProductImage {
                url: format!(
                    "/api/published/{}/images/{}/{}/{}",
                    p.locale.public(),
                    p.sku,
                    p.publication_revision,
                    r.edge
                ),
                edge: r.edge,
                width: r.width,
                height: r.height,
            })
            .collect();
        Self {
            minor_units: p.price.minor_units(),
            currency: "EUR".into(),
            price_decimal: format!(
                "{}.{:02}",
                p.price.minor_units() / 100,
                p.price.minor_units() % 100
            ),
            images,
            components: p
                .components
                .into_iter()
                .map(|c| Component {
                    code: c.code,
                    quantity: c.quantity,
                    dimensions: measurements(c.dimensions),
                    material: match c.material {
                        source::PublishedMaterial::Cover { fibres } => Material::Cover {
                            fibres: fibres
                                .into_iter()
                                .map(|f| Fibre {
                                    name: f.name,
                                    basis_points: f.basis_points,
                                })
                                .collect(),
                        },
                        source::PublishedMaterial::Foam { density_g_per_m3 } => {
                            Material::Foam { density_g_per_m3 }
                        }
                    },
                })
                .collect(),
            parcels: p
                .parcels
                .into_iter()
                .map(|p| Parcel {
                    dimensions: measurements(p.dimensions),
                    gross_weight_g: p.gross_weight_g,
                })
                .collect(),
            footprint: measurements(p.footprint),
            copy: ProductCopy {
                title: p.copy.title,
                description: p.copy.description,
                materials: p.copy.materials,
                care: p.copy.care,
                safety: p.copy.safety,
                assembly: p.copy.assembly,
            },
            availability: if p.in_stock {
                Availability::InStock
            } else {
                Availability::OutOfStock
            },
            sku: p.sku,
            locale: p.locale.public().into(),
            publication_revision: p.publication_revision,
            family_code: p.family_code,
            family_slug: p.family_slug,
            size_code: p.size_code,
            textile_code: p.textile_code,
            fill: p.fill,
        }
    }
}

#[derive(ToSchema)]
#[schema(value_type=String,format=Binary)]
pub struct ImageBytes(pub Vec<u8>);
