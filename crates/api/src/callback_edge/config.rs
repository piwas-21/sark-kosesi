//! Explicit edge configuration; disabled mode installs no listener or credentials.
use axum::http::HeaderValue;
use commerce_infrastructure::callback_forwarder::CallbackForwarder;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
};
pub struct Config {
    pub(super) bind: SocketAddr,
    pub(super) peers: Vec<IpAddr>,
    pub(super) forwarder: CallbackForwarder,
    revision: Option<String>,
    pub(super) revision_header: Option<HeaderValue>,
}
impl Config {
    pub fn parse(values: &HashMap<String, String>) -> Result<Option<Self>, &'static str> {
        let environment = values.get("APP_ENV").map(String::as_str);
        if environment == Some("production") {
            return Err("production is disabled");
        }
        match values.get("CALLBACK_EDGE_ENABLED").map(String::as_str) {
            None | Some("false") => return Ok(None),
            Some("true") => {}
            _ => return Err("invalid CALLBACK_EDGE_ENABLED"),
        }
        if !matches!(environment, Some("local" | "test" | "staging")) {
            return Err("invalid callback edge environment");
        }
        let staging = environment == Some("staging");
        let bind: SocketAddr = required(values, "CALLBACK_EDGE_BIND")?
            .parse()
            .map_err(|_| "invalid CALLBACK_EDGE_BIND")?;
        if (staging && bind.port() != 45672) || (!staging && !bind.ip().is_loopback()) {
            return Err("invalid CALLBACK_EDGE_BIND");
        }
        let mut peers = Vec::new();
        for raw in required(values, "CALLBACK_EDGE_PROXY_IPS")?.split(',') {
            let ip: IpAddr = raw
                .trim()
                .parse()
                .map_err(|_| "invalid CALLBACK_EDGE_PROXY_IPS")?;
            if peers.len() >= 4
                || peers.contains(&ip)
                || !(private(ip) || (!staging && ip.is_loopback()))
            {
                return Err("invalid CALLBACK_EDGE_PROXY_IPS");
            }
            peers.push(ip);
        }
        let revision = values.get("PREVIEW_REVISION").cloned();
        if (staging && revision.is_none())
            || revision.as_ref().is_some_and(|v| {
                v.len() != 40
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err("invalid PREVIEW_REVISION");
        }
        let revision_header = if staging {
            revision
                .as_deref()
                .map(HeaderValue::from_str)
                .transpose()
                .map_err(|_| "invalid PREVIEW_REVISION")?
        } else {
            None
        };
        let forwarder = CallbackForwarder::new(
            required(values, "API_URL")?,
            required(values, "PREVIEW_TOKEN")?,
        )
        .map_err(|_| "invalid callback edge upstream configuration")?;
        Ok(Some(Self {
            bind,
            peers,
            forwarder,
            revision,
            revision_header,
        }))
    }
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }
    /// Read only known settings. Non-Unicode secrets produce static errors, not a
    /// panic or an OsString diagnostic containing credential bytes.
    pub fn from_env() -> Result<Option<Self>, &'static str> {
        let mut values = HashMap::new();
        for key in [
            "APP_ENV",
            "CALLBACK_EDGE_ENABLED",
            "CALLBACK_EDGE_BIND",
            "CALLBACK_EDGE_PROXY_IPS",
            "API_URL",
            "PREVIEW_TOKEN",
            "PREVIEW_REVISION",
        ] {
            match std::env::var(key) {
                Ok(value) => {
                    values.insert(key.to_owned(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(_) => return Err("non-Unicode callback edge configuration"),
            }
        }
        Self::parse(&values)
    }
}
fn required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str, &'static str> {
    values
        .get(key)
        .map(String::as_str)
        .ok_or("callback edge configuration required")
}
fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(ip) => ip.is_unique_local(),
    }
}
