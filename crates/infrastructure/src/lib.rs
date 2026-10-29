//! Parameterized PostgreSQL catalogue adapter and forward migration owner.
#![forbid(unsafe_code)]

pub mod bom;
pub mod catalogue_authoring;
pub mod catalogue_prices;
pub mod factual_review;
pub mod holds;
pub mod image_ingestion;
pub mod media;
pub mod runtime;
pub mod specifications;
pub mod staff_access;
pub mod staff_operations;
pub mod stock;
pub mod translations;

use commerce_application::Locale;
use commerce_domain::{CatalogueItem, Money};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_millis(250))
        .connect(database_url)
        .await
}

pub async fn readiness(pool: &PgPool) -> Result<(), sqlx::Error> {
    let result = tokio::time::timeout(Duration::from_millis(300), async {
        sqlx::query("SELECT sku FROM catalogue_items LIMIT 1")
            .execute(pool)
            .await?;
        sqlx::query("SELECT care FROM catalogue_translations LIMIT 1")
            .execute(pool)
            .await?;
        Ok(())
    })
    .await;
    result.map_err(|_| sqlx::Error::PoolTimedOut)?
}

#[derive(sqlx::FromRow)]
struct CatalogueRow {
    sku: String,
    slug: String,
    title: String,
    description: String,
    care: String,
    minor_units: i64,
    fill: String,
    fixture: bool,
}

pub async fn catalogue(pool: &PgPool, locale: Locale) -> Result<Vec<CatalogueItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, CatalogueRow>("SELECT i.sku, i.slug, i.minor_units, i.fill, i.fixture, t.title, t.description, t.care FROM catalogue_items i JOIN catalogue_translations t USING (sku) WHERE i.fixture AND t.locale = $1 ORDER BY i.sku LIMIT 50")
        .bind(locale.internal()).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            let price = Money::eur(row.minor_units)
                .map_err(|_| sqlx::Error::Protocol("Invalid stored money".into()))?;
            Ok(CatalogueItem {
                sku: row.sku,
                slug: row.slug,
                title: row.title,
                description: row.description,
                care: row.care,
                price,
                fill: row.fill,
                fixture: row.fixture,
            })
        })
        .collect()
}
