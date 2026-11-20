//! Whom a request trusts, for a real process: the authorities that
//! may say who a server is, and where the server is found. See
//! "Network" in `docs/spec/targets.md`.

use std::net::{IpAddr, SocketAddr};

use ureq::Agent;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{PemItem, RootCerts, TlsConfig, parse_pem};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use super::{Failure, Kind};

/// Whom a request trusts in place of whom the world does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Trust {
    /// Certificates, as PEM, of the only authorities trusted to say
    /// who a server is.
    pub roots: Option<String>,
    /// The address the server is at, whatever its name leads to. Who
    /// it must show it is stays the name in the URL.
    pub address: Option<String>,
}

fn invalid(message: &dyn ToString) -> Failure {
    Failure {
        kind: Kind::Invalid,
        message: message.to_string(),
    }
}

/// Finds every host at one address.
#[derive(Debug)]
struct At(IpAddr);

impl Resolver for At {
    fn resolve(
        &self,
        uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let secure = uri.scheme_str() == Some("https");
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let mut found = self.empty();
        found.push(SocketAddr::new(self.0, port));
        Ok(found)
    }
}

/// What a request is configured with, whomever it trusts.
pub(super) fn config(tls: TlsConfig) -> Config {
    let config = Agent::config_builder().http_status_as_error(false);
    config.tls_config(tls).build()
}

/// What sends the requests that trust so.
pub(super) fn agent(trust: &Trust) -> Result<Agent, Failure> {
    let mut tls = TlsConfig::builder();
    if let Some(roots) = &trust.roots {
        let certificate = |item| match item {
            Ok(PemItem::Certificate(certificate)) => Some(Ok(certificate)),
            Ok(_) => None,
            Err(error) => Some(Err(invalid(&error))),
        };
        let found: Result<Vec<_>, Failure> = parse_pem(roots.as_bytes())
            .filter_map(certificate)
            .collect();
        let found = found?;
        if found.is_empty() {
            return Err(invalid(&"the roots hold no certificate"));
        }
        tls = tls.root_certs(RootCerts::new_with_certs(&found));
    }
    let config = config(tls.build());
    let Some(address) = &trust.address else {
        return Ok(config.into());
    };
    let address = address
        .parse()
        .map_err(|_| invalid(&format!("`{address}` is no address")))?;
    Ok(Agent::with_parts(
        config,
        DefaultConnector::new(),
        At(address),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Kind, Trust, agent};

    #[test]
    fn roots_that_hold_no_certificate_and_an_address_that_is_none_are_refused() {
        let roots = Trust {
            roots: Some("not a certificate".to_owned()),
            address: None,
        };
        let address = Trust {
            roots: None,
            address: Some("bridge.local".to_owned()),
        };

        let refused = |trust| agent(&trust).map(|_| ()).map_err(|failure| failure.kind);
        assert_eq!(refused(roots), Err(Kind::Invalid));
        assert_eq!(refused(address), Err(Kind::Invalid));
        assert_eq!(refused(Trust::default()), Ok(()));
    }
}
