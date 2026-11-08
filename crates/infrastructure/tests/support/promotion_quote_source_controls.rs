use super::*;
use crate::MIGRATOR;

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_or_missing_promotion_references_cannot_supply_quote_authority(pool: PgPool) {
    let mut tx = pool.begin().await.expect("isolated source transaction");
    assert!(matches!(
        current_for_quote(&mut tx, "invalid promotion").await,
        Err(PolicyError::NotFound)
    ));
    assert!(matches!(
        current_for_quote(&mut tx, "UNINSTALLED").await,
        Err(PolicyError::NotFound)
    ));
    for (id, draft, review) in [
        ("invalid promotion", 1, 1),
        ("UNINSTALLED", 0, 1),
        ("UNINSTALLED", 1, 0),
        ("UNINSTALLED", 1, 1),
    ] {
        assert!(matches!(
            historical_for_quote(&mut tx, id, draft, review).await,
            Err(PolicyError::InvalidStoredData)
        ));
    }
    tx.rollback().await.expect("read only");
}
