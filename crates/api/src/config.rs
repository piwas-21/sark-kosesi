use commerce_infrastructure::image_ingestion::DecoderEndpoint;
use std::{collections::HashMap, net::SocketAddr};

/// Capability created only by validated test-mode configuration.
#[derive(Clone)]
pub struct TestGuestCancellation(());

/// Capability created only by the separately validated pending-request flag.
#[derive(Clone)]
pub struct TestGuestPendingCancellation(());

pub struct Config {
    pub database_url: String,
    pub preview_database_preapplied: bool,
    pub preview_token: String,
    pub bind: SocketAddr,
    pub image_decoder: Option<DecoderEndpoint>,
    pub guest_origin: Option<crate::guest::GuestOrigin>,
    pub test_guest_cancellation: Option<TestGuestCancellation>,
    pub test_guest_pending_cancellation: Option<TestGuestPendingCancellation>,
    pub verification: Option<crate::verification::VerificationConfig>,
    pub test_login: Option<crate::test_login::LoginConfig>,
    pub test_guest_order: Option<crate::guest::test_orders::TestOrderConfig>,
    pub callback_profile: Option<crate::callbacks::CallbackProfile>,
}

impl Config {
    pub fn parse(values: &HashMap<String, String>) -> Result<Self, &'static str> {
        let environment = values.get("APP_ENV").map(String::as_str);
        if !matches!(environment, Some("local" | "test" | "staging")) {
            return Err("APP_ENV must be local, test or staging; production is disabled");
        }
        let preview_database_preapplied = match values
            .get("PREVIEW_DATABASE_PREAPPLIED")
            .map(String::as_str)
        {
            None | Some("0") => false,
            Some("1") if environment == Some("staging") => true,
            _ => return Err("PREVIEW_DATABASE_PREAPPLIED requires staging and must be 0 or 1"),
        };
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
        let image_decoder = values
            .get("PREVIEW_IMAGE_DECODER_SOCKET")
            .map(|value| DecoderEndpoint::parse(value))
            .transpose()?;
        let guest_origin = values
            .get("GUEST_ORIGIN")
            .map(|value| crate::guest::GuestOrigin::parse(value))
            .transpose()?;
        let test_guest_order =
            crate::guest::test_orders::TestOrderConfig::from_env(environment, values)?;
        if test_guest_order.is_some() && guest_origin.is_none() {
            return Err("Test guest order requires guest origin");
        }
        let test_guest_cancellation = match values
            .get("TEST_GUEST_CANCELLATION_TRANSPORT")
            .map(String::as_str)
        {
            None => None,
            Some("1") if environment == Some("test") && guest_origin.is_some() => {
                Some(TestGuestCancellation(()))
            }
            _ => return Err("Test guest cancellation requires test mode and GUEST_ORIGIN"),
        };
        let test_guest_pending_cancellation = match values
            .get("TEST_GUEST_PENDING_CANCELLATION_TRANSPORT")
            .map(String::as_str)
        {
            None => None,
            Some("1") if environment == Some("test") && guest_origin.is_some() => {
                Some(TestGuestPendingCancellation(()))
            }
            _ => return Err("Test guest pending cancellation requires test mode and GUEST_ORIGIN"),
        };
        let callback_profile = values
            .get("CALLBACK_INTAKE_PROFILE_ID")
            .map(|value| crate::callbacks::CallbackProfile::parse(value))
            .transpose()?;
        let verification = crate::verification::VerificationConfig::from_env(environment, values)?;
        let test_login = crate::test_login::LoginConfig::from_env(environment, values)?;
        if preview_database_preapplied
            && (guest_origin.is_some()
                || callback_profile.is_some()
                || verification.is_some()
                || test_login.is_some())
        {
            return Err("Preapplied preview cannot enable writing transports");
        }
        if test_login.is_some()
            && verification.is_some()
            && values.get("TEST_LOGIN_ADMISSION_KEY")
                == values.get("TEST_VERIFICATION_ADMISSION_KEY")
        {
            return Err("Test login and verification require separate admission keys");
        }
        Ok(Self {
            database_url,
            preview_database_preapplied,
            preview_token,
            bind,
            image_decoder,
            guest_origin,
            test_guest_cancellation,
            test_guest_pending_cancellation,
            callback_profile,
            verification,
            test_login,
            test_guest_order,
        })
    }
}
