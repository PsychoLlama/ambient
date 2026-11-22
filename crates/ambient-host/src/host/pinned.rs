//! A connection secured with a server that is held to one certificate,
//! whatever name the certificate is for and whoever signed it, and
//! the certificate a server shows, to learn which to hold it to. See
//! "Network" in `docs/spec/targets.md`.
//!
//! The client this host sends with asks an authority or asks no one,
//! so this is the part of it that secures a connection, written again
//! over a verifier of our own. The server still proves it holds the
//! certificate's key: only who vouches for the certificate changes.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, PoisonError};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct};
use rustls::{SignatureScheme, StreamOwned};
use sha2::{Digest, Sha256};
use ureq::Error;
use ureq::http::Uri;
use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, Either, LazyBuffers};
use ureq::unversioned::transport::{NextTimeout, Transport, TransportAdapter};

use super::{Failure, Kind};

/// The SHA-256 of a certificate: what a pin is.
pub(super) type Digest256 = [u8; 32];

/// Which certificate a server may show.
#[derive(Debug)]
enum Wanted {
    /// The one of this digest.
    Pinned(Digest256),
    /// Any, which is kept to be read once the server proves it holds
    /// its key.
    Any(Arc<Mutex<Option<Vec<u8>>>>),
}

/// Trusts the certificate it wants, for any name and at any time: the
/// program said which certificate, or asked only what it is, so no
/// authority and no date is asked.
#[derive(Debug)]
struct Shown {
    wanted: Wanted,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Shown {
    fn verify_server_cert(
        &self,
        shown: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.wanted {
            Wanted::Pinned(digest) if Sha256::digest(shown.as_ref()).as_slice() == digest => {
                Ok(ServerCertVerified::assertion())
            }
            Wanted::Pinned(_) => Err(CertificateError::ApplicationVerificationFailure.into()),
            Wanted::Any(kept) => {
                let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
                *kept = Some(shown.as_ref().to_vec());
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls12_signature(message, certificate, signed, algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls13_signature(message, certificate, signed, algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Secures each connection it is handed with a server that shows the
/// pinned certificate, in place of the client's own way to.
#[derive(Debug)]
pub(super) struct Pinning(Arc<ClientConfig>);

impl Pinning {
    /// What secures connections with the server whose certificate has
    /// `digest`, or why nothing can.
    pub(super) fn new(digest: Digest256) -> Result<Self, rustls::Error> {
        Ok(Self(secured_by(Wanted::Pinned(digest))?))
    }
}

/// What secures a connection with a server that shows what is wanted.
fn secured_by(wanted: Wanted) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let shown = Arc::new(Shown {
        wanted,
        provider: Arc::clone(&provider),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(shown)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn failed(kind: Kind, message: &dyn ToString) -> Failure {
    Failure {
        kind,
        message: message.to_string(),
    }
}

/// The certificate the server at `url` shows, as DER, once it proves
/// it holds the certificate's key: trusted or not, for whatever name.
/// `address` is where the server is, or nothing to look the host up.
///
/// It blocks until the connection is secured or fails, so it is called
/// on a thread of its own.
pub(super) fn certificate(url: &str, address: Option<&str>) -> Result<Vec<u8>, Failure> {
    let uri: Uri = url.parse().map_err(|error| failed(Kind::Invalid, &error))?;
    if uri.scheme_str() != Some("https") {
        return Err(failed(
            Kind::Invalid,
            &"only an `https` URL shows a certificate",
        ));
    }
    let host = uri.host().unwrap_or_default();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = uri.port_u16().unwrap_or(443);
    let found: Vec<SocketAddr> = match address {
        Some(address) => {
            let address = address.parse();
            vec![SocketAddr::new(
                address.map_err(|_| failed(Kind::Invalid, &"the address is none"))?,
                port,
            )]
        }
        None => (host, port)
            .to_socket_addrs()
            .map_err(|error| failed(Kind::Unreachable, &error))?
            .collect(),
    };
    let mut socket =
        TcpStream::connect(&found[..]).map_err(|error| failed(Kind::Unreachable, &error))?;
    let name = ServerName::try_from(host.to_owned()).map_err(|_| {
        failed(
            Kind::Invalid,
            &"the URL's host is no name to ask a server for",
        )
    })?;
    let untrusted = |error: &dyn ToString| failed(Kind::Untrusted, error);
    let kept = Arc::new(Mutex::new(None));
    let config = secured_by(Wanted::Any(Arc::clone(&kept))).map_err(|error| untrusted(&error))?;
    let mut connection = ClientConnection::new(config, name).map_err(|error| untrusted(&error))?;
    while connection.is_handshaking() {
        connection
            .complete_io(&mut socket)
            .map_err(|error| untrusted(&error))?;
    }
    connection.send_close_notify();
    let _ = connection.complete_io(&mut socket);
    let kept = kept.lock().unwrap_or_else(PoisonError::into_inner).take();
    kept.ok_or_else(|| untrusted(&"the server showed no certificate"))
}

impl<In: Transport> Connector<In> for Pinning {
    type Out = Either<In, Secured>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, Error> {
        let Some(transport) = chained else {
            return Ok(None);
        };
        if !details.needs_tls() || transport.is_tls() {
            return Ok(Some(Either::A(transport)));
        }
        // The name is what the server is asked for, and decides
        // nothing: an address is one too.
        let host = details.uri.host().unwrap_or_default();
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let name = ServerName::try_from(host.to_owned())
            .map_err(|_| Error::Tls("the URL's host is no name to ask a server for"))?;
        let mut connection = ClientConnection::new(Arc::clone(&self.0), name)?;
        let mut socket = TransportAdapter::new(transport.boxed(), details.current_time.clone());
        socket.set_timeout(details.timeout);
        connection.complete_io(&mut socket)?;
        let config = details.config;
        Ok(Some(Either::B(Secured {
            buffers: LazyBuffers::new(config.input_buffer_size(), config.output_buffer_size()),
            stream: StreamOwned::new(connection, socket),
        })))
    }
}

/// A connection [`Pinning`] secured.
pub(super) struct Secured {
    buffers: LazyBuffers,
    stream: StreamOwned<ClientConnection, TransportAdapter>,
}

impl std::fmt::Debug for Secured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secured").finish_non_exhaustive()
    }
}

impl Transport for Secured {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), Error> {
        self.stream.get_mut().set_timeout(timeout);
        let output = &self.buffers.output()[..amount];
        Ok(self.stream.write_all(output)?)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, Error> {
        self.stream.get_mut().set_timeout(timeout);
        let amount = self.stream.read(self.buffers.input_append_buf())?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        self.stream.get_mut().get_mut().is_open()
    }

    fn is_tls(&self) -> bool {
        true
    }
}
