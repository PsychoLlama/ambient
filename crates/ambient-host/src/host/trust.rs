//! Whom a request trusts, for a real process: the authorities that
//! may say who a server is, and where the server is found. See
//! "Network" in `docs/spec/targets.md`.

use std::net::{IpAddr, SocketAddr};

use ureq::Agent;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{PemItem, RootCerts, TlsConfig, parse_pem};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{Connector, DefaultConnector, NextTimeout, TcpConnector};

use super::pinned::{Digest256, Pinning};
use super::{Failure, Kind};

/// Whom a request trusts in place of whom the world does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Trust {
    /// Certificates, as PEM, of the only authorities trusted to say
    /// who a server is.
    pub roots: Option<String>,
    /// The SHA-256 of the one certificate the server must show, in
    /// place of any authority and any name.
    pub pin: Option<Vec<u8>>,
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

/// Where a request's server is looked for.
enum Found {
    Looked(DefaultResolver),
    At(At),
}

impl std::fmt::Debug for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Found").finish_non_exhaustive()
    }
}

impl Resolver for Found {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        match self {
            Self::Looked(resolver) => resolver.resolve(uri, config, timeout),
            Self::At(at) => at.resolve(uri, config, timeout),
        }
    }
}

/// What sends the requests that trust so.
pub(super) fn agent(trust: &Trust) -> Result<Agent, Failure> {
    let found = match &trust.address {
        None => Found::Looked(DefaultResolver::default()),
        Some(address) => {
            let address = address.parse();
            Found::At(At(address.map_err(|_| invalid(&"the address is none"))?))
        }
    };
    let Some(pin) = &trust.pin else {
        let config = config(authorities(trust)?);
        return Ok(Agent::with_parts(config, DefaultConnector::new(), found));
    };
    if trust.roots.is_some() {
        return Err(invalid(
            &"a pin asks no authority, so roots beside one mean nothing",
        ));
    }
    let digest = Digest256::try_from(pin.as_slice())
        .map_err(|_| invalid(&"a pin is the 32 bytes of a certificate's SHA-256"))?;
    let pinning = Pinning::new(digest).map_err(|error| invalid(&error))?;
    let connector = TcpConnector::default().chain(pinning);
    Ok(Agent::with_parts(
        config(TlsConfig::default()),
        connector,
        found,
    ))
}

/// The authorities a request that pins nothing asks: its roots, or
/// those the world trusts.
fn authorities(trust: &Trust) -> Result<TlsConfig, Failure> {
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
    Ok(tls.build())
}

#[cfg(test)]
mod tests {
    use super::{Kind, Trust, agent};

    #[test]
    fn trust_that_cannot_be_read_is_refused() {
        let roots = Trust {
            roots: Some("not a certificate".to_owned()),
            ..Trust::default()
        };
        let address = Trust {
            address: Some("bridge.local".to_owned()),
            ..Trust::default()
        };
        let short = Trust {
            pin: Some(vec![0; 20]),
            ..Trust::default()
        };
        let twice = Trust {
            pin: Some(vec![0; 32]),
            ..roots.clone()
        };

        let refused = |trust| agent(&trust).map(|_| ()).map_err(|failure| failure.kind);
        assert_eq!(refused(roots), Err(Kind::Invalid));
        assert_eq!(refused(address), Err(Kind::Invalid));
        assert_eq!(refused(short), Err(Kind::Invalid));
        assert_eq!(refused(twice), Err(Kind::Invalid));
        assert_eq!(refused(Trust::default()), Ok(()));
    }
}
