use crate::{ApiError, Catalogue, Product};
use utoipa::{
    Modify, OpenApi,
    openapi::{
        OpenApi as Document,
        security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme},
    },
};

#[derive(OpenApi)]
#[openapi(paths(crate::test_linked_orders::list,crate::test_favourites::list,crate::test_session::login,crate::test_session::logout,crate::test_login::login,crate::verification::redeem,crate::callbacks::intake,crate::get_catalogue,crate::published::page,crate::published::product,crate::published::image,crate::guest::issue,crate::guest::read,crate::guest::write,crate::guest::revoke,crate::guest::quotes::issue,crate::guest::quotes::read), components(schemas(crate::callbacks::CallbackForm,Catalogue, Product, ApiError, crate::published_types::PublishedCatalogue, crate::published_types::PublishedProduct)), modifiers(&PreviewAuth))]
pub struct ApiDoc;
struct PreviewAuth;
impl Modify for PreviewAuth {
    fn modify(&self, document: &mut Document) {
        if let Some(components) = document.components.as_mut() {
            components.add_security_scheme(
                "guest",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("__Host-piwas-guest"))),
            );
            components.add_security_scheme(
                "customer_test",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("__Host-piwas-customer"))),
            );
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
