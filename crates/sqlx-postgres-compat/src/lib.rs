//! PostgreSQL-only exports from the official SQLx 0.9 crates.

pub use sqlx_core::acquire::Acquire;
pub use sqlx_core::column::ColumnIndex;
pub use sqlx_core::connection::ConnectOptions;
pub use sqlx_core::error::{Error, Result};
pub use sqlx_core::executor::Executor;
pub use sqlx_core::from_row::FromRow;
pub use sqlx_core::query::query;
pub use sqlx_core::query_as::query_as;
pub use sqlx_core::query_scalar::query_scalar;
pub use sqlx_core::raw_sql::raw_sql;
pub use sqlx_core::row::Row;
pub use sqlx_core::sql_str::{AssertSqlSafe, SqlStr};
pub use sqlx_core::transaction::Transaction;
pub use sqlx_core::{decode, types};
pub use sqlx_core::{migrate, testing};
#[doc(hidden)]
pub use sqlx_macros;
pub use sqlx_macros::{FromRow, test};
pub use sqlx_postgres::{self as postgres, PgConnection, PgPool, Postgres};

#[macro_export]
macro_rules! migrate {
    ($dir:literal) => {
        $crate::sqlx_macros::migrate!($dir)
    };
    () => {
        $crate::sqlx_macros::migrate!()
    };
}
