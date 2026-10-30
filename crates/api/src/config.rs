use std::{collections::HashMap, net::SocketAddr};

pub struct Config {
    pub database_url: String,
    pub preview_token: String,
    pub bind: SocketAddr,
}

impl Config {
    pub fn parse(values: &HashMap<String, String>) -> Result<Self, &'static str> {
        let environment = values.get("APP_ENV").map(String::as_str);
        if !matches!(environment, Some("local" | "test" | "staging")) {
            return Err("APP_ENV must be local, test or staging; production is disabled");
        }
        let database_url = values
            .get("DATABASE_URL")
            .ok_or("DATABASE_URL required")?
            .clone();
        if !(database_url.starts_with("postgresql://") || database_url.starts_with("postgres://"))
            || database_url
                .parse::<sqlx::postgres::PgConnectOptions>()
                .is_err()
        {
            return Err("PostgreSQL URL required");
        }
        let preview_token = values
            .get("PREVIEW_TOKEN")
            .ok_or("PREVIEW_TOKEN required")?
            .clone();
        if !(32..=128).contains(&preview_token.len())
            || !preview_token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("PREVIEW_TOKEN must be 32-128 URL-safe characters");
        }
        let bind: SocketAddr = values
            .get("API_BIND")
            .ok_or("API_BIND required")?
            .parse()
            .map_err(|_| "Invalid API_BIND")?;
        if !bind.ip().is_loopback() {
            return Err("Preview API must bind loopback");
        }
        Ok(Self {
            database_url,
            preview_token,
            bind,
        })
    }
}
