// SPDX-License-Identifier: MIT
//! One rule for every HTTP request that leaves this program: may it leave the machine at all
//!
//! Two clients send a request to an address that the settings name: the language model client and
//! the remote recognizer. Both ask their question here, so the answer cannot differ between the
//! two, and a redirect cannot slip past one of them because the other one was written later
//!
//! The rule itself is short. A loopback address is this machine, so a request there always goes
//! out. Any other address names a provider that bills and keeps a copy, so it needs
//! `privacy.cloud_consent`. Without that consent the client also takes no proxy: a proxy is
//! another machine, and the request would leave through it while the address looks local

use std::time::Duration;

use thiserror::Error;

/// The consent of `privacy.cloud_consent`, and every HTTP decision that follows from it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpPolicy {
    cloud_consent: bool,
}

/// Why a request did not leave, or did not go further
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConsentError {
    #[error("the request would leave this machine, and privacy.cloud_consent is off")]
    Cloud,
    #[error("the answer redirects to another machine, and privacy.cloud_consent is off")]
    RemoteRedirect,
    #[error("the answer redirects within this machine, and a redirect is not followed")]
    LocalRedirect,
}

impl HttpPolicy {
    pub fn new(cloud_consent: bool) -> Self {
        Self { cloud_consent }
    }

    pub fn cloud_consent(self) -> bool {
        self.cloud_consent
    }

    /// Whether a URL may be contacted under this policy
    pub fn allows(self, url: &str) -> bool {
        self.cloud_consent || is_loopback_url(url)
    }

    /// Refuse a URL that this policy does not allow
    pub fn guard(self, url: &str) -> Result<(), ConsentError> {
        if self.allows(url) {
            Ok(())
        } else {
            Err(ConsentError::Cloud)
        }
    }

    /// Client that keeps this policy: without consent neither a proxy nor a redirect, and the
    /// local names resolve to this machine, so no DNS answer can move them elsewhere
    pub fn client_builder(self, timeout: Duration) -> reqwest::blocking::ClientBuilder {
        let builder = reqwest::blocking::Client::builder().timeout(timeout);
        if self.cloud_consent {
            return builder;
        }
        builder
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(
                "localhost",
                &[
                    ([127, 0, 0, 1], 0).into(),
                    (std::net::Ipv6Addr::LOCALHOST, 0).into(),
                ],
            )
    }

    /// The same builder, built
    pub fn client(self, timeout: Duration) -> Result<reqwest::blocking::Client, reqwest::Error> {
        self.client_builder(timeout).build()
    }

    /// Check an answer before its body is read
    ///
    /// A client built without consent follows no redirect, so a 3xx reaches the caller. A
    /// redirect abroad is the same request to another machine and needs the consent the settings
    /// do not carry; a redirect that stays here still names an address the settings do not, and
    /// is refused
    pub fn guard_response(
        self,
        response: &reqwest::blocking::Response,
    ) -> Result<(), ConsentError> {
        if self.cloud_consent || !response.status().is_redirection() {
            return Ok(());
        }
        let target = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|location| response.url().join(location).ok());
        match target {
            Some(url) if is_loopback_url(url.as_str()) => Err(ConsentError::LocalRedirect),
            _ => Err(ConsentError::RemoteRedirect),
        }
    }
}

/// Error text together with the `source()` chain
///
/// reqwest writes only "builder error" or "error sending request" at the top, and keeps the
/// reason that lets a user fix something — no certificates, connection refused — in the source
pub fn with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

/// Whether a URL names a loopback address
///
/// A name that only looks local is not: `127.evil.example` and a host with credentials resolve to
/// another machine, and a port that does not parse names no address at all
pub fn is_loopback_url(base_url: &str) -> bool {
    let input = base_url.trim();
    let input = if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };
    let Ok(url) = reqwest::Url::parse(&input) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_address_needs_no_consent() {
        let policy = HttpPolicy::new(false);
        for url in [
            "http://127.0.0.1:8080/v1",
            "http://localhost:8000/v1",
            "http://[::1]:11434/v1",
        ] {
            assert_eq!(policy.guard(url), Ok(()), "{url}");
            assert!(policy.allows(url), "{url}");
        }
    }

    #[test]
    fn another_machine_needs_the_consent_of_the_settings() {
        let url = "https://api.groq.com/openai/v1";
        assert_eq!(HttpPolicy::new(false).guard(url), Err(ConsentError::Cloud));
        assert_eq!(HttpPolicy::new(true).guard(url), Ok(()));
    }

    #[test]
    fn a_spoofed_local_name_is_another_machine() {
        let policy = HttpPolicy::new(false);
        for url in [
            "http://127.evil.example/v1",
            "http://0.0.0.0/v1",
            "http://localhost@remote.example/v1",
        ] {
            assert_eq!(policy.guard(url), Err(ConsentError::Cloud), "{url}");
        }
    }
}
