use crate::{ApiError, Catalogue, Product};
use utoipa::{
    Modify, OpenApi,
    openapi::{
        OpenApi as Document,
        security::{Http, HttpAuthScheme, SecurityScheme},
    },
};

#[derive(OpenApi)]
#[openapi(paths(crate::get_catalogue,crate::published::page,crate::published::product,crate::published::image), components(schemas(Catalogue, Product, ApiError, crate::published_types::PublishedCatalogue, crate::published_types::PublishedProduct)), modifiers(&PreviewAuth))]
pub struct ApiDoc;
struct PreviewAuth;
impl Modify for PreviewAuth {
    fn modify(&self, document: &mut Document) {
        if let Some(components) = document.components.as_mut() {
            components.add_security_scheme(
                "preview",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
        }
    }
}
pub fn openapi() -> Result<String, serde_json::Error> {
    ApiDoc::openapi().to_pretty_json()
}
