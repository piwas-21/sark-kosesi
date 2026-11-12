use std::{collections::HashMap, net::SocketAddr};

pub struct Config {
    pub database_url: String,
    pub preview_token: String,
    pub bind: SocketAddr,
    pub guest_origin: Option<crate::guest::GuestOrigin>,
    pub verification: Option<crate::verification::VerificationConfig>,
    pub test_login: Option<crate::test_login::LoginConfig>,
    pub callback_profile: Option<crate::callbacks::CallbackProfile>,
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
        let guest_origin = values
            .get("GUEST_ORIGIN")
            .map(|value| crate::guest::GuestOrigin::parse(value))
            .transpose()?;
        let callback_profile = values
            .get("CALLBACK_INTAKE_PROFILE_ID")
            .map(|value| crate::callbacks::CallbackProfile::parse(value))
            .transpose()?;
        let verification = crate::verification::VerificationConfig::from_env(environment, values)?;
        let test_login = crate::test_login::LoginConfig::from_env(environment, values)?;
        if test_login.is_some()
            && verification.is_some()
            && values.get("TEST_LOGIN_ADMISSION_KEY")
                == values.get("TEST_VERIFICATION_ADMISSION_KEY")
        {
            return Err("Test login and verification require separate admission keys");
        }
        Ok(Self {
            database_url,
            preview_token,
            bind,
            guest_origin,
            callback_profile,
            verification,
            test_login,
        })
    }
}
