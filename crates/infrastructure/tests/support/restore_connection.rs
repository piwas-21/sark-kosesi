//! Fail-closed URL guard for exclusively owned restore-test databases.
use std::net::Ipv4Addr;

pub(super) fn is_owned_url(raw: &str, database: &str) -> bool {
    if !database.starts_with("sark_restore_")
        || database.len() != "sark_restore_".len() + 32
        || !database["sark_restore_".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    parsed.scheme() == "postgresql"
        && parsed.username() == "p1_test"
        && parsed
            .password()
            .is_some_and(|password| !password.is_empty())
        && parsed.host_str() == Some("127.0.0.1")
        && parsed
            .host_str()
            .and_then(|host| host.parse::<Ipv4Addr>().ok())
            == Some(Ipv4Addr::LOCALHOST)
        && parsed.port().is_some_and(|port| port != 0)
        && parsed.path() == format!("/{database}")
        && parsed.query().is_none()
        && parsed.fragment().is_none()
}

#[cfg(test)]
mod tests {
    use super::is_owned_url;

    #[test]
    fn rejects_host_spoofing_and_overrides() {
        let database = format!("sark_restore_{}", "a".repeat(32));
        assert!(is_owned_url(
            &format!("postgresql://p1_test:fixture@127.0.0.1:1234/{database}"),
            &database
        ));
        for url in [
            format!("postgresql://p1_test:pw@127.0.0.1:1234@evil.example:1234/{database}"),
            format!("postgresql://p1_test:pw@evil.example:1234/{database}"),
            format!("postgresql://p1_test:pw@127.0.0.1:1234/{database}?host=evil.example"),
            format!("postgresql://p1_test:pw@127.0.0.1:1234/{database}#fragment"),
            format!("postgresql://p1_test:pw@127.0.0.1:0/{database}"),
            format!("postgresql://p1_test:pw@127.0.0.1/{database}"),
            format!("postgresql://p1_test:pw@[::1]:1234/{database}"),
            format!("postgresql://other:pw@127.0.0.1:1234/{database}"),
            format!("postgresql://p1_test:pw@127.0.0.1:1234/{database}/other"),
        ] {
            assert!(!is_owned_url(&url, &database));
        }
        assert!(!is_owned_url(
            "postgresql://p1_test:pw@127.0.0.1:1234/postgres",
            "postgres"
        ));
    }
}
