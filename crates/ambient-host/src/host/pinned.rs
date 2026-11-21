//! A connection secured with a server that is held to one certificate,
//! whatever name the certificate is for and whoever signed it. See
//! "Network" in `docs/spec/targets.md`.
//!
//! The client this host sends with asks an authority or asks no one,
//! so this is the part of it that secures a connection, written again
//! over a verifier of our own. The server still proves it holds the
//! certificate's key: only who vouches for the certificate changes.

use std::io::{Read, Write};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct};
use rustls::{SignatureScheme, StreamOwned};
use sha2::{Digest, Sha256};
use ureq::Error;
use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, Either, LazyBuffers};
use ureq::unversioned::transport::{NextTimeout, Transport, TransportAdapter};

/// The SHA-256 of a certificate: what a pin is.
pub(super) type Digest256 = [u8; 32];

/// Trusts the one certificate of this digest, for any name and at any
/// time: the program said which certificate, so no authority and no
/// date is asked.
#[derive(Debug)]
struct Shown {
    digest: Digest256,
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
        if Sha256::digest(shown.as_ref()).as_slice() == self.digest {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(CertificateError::ApplicationVerificationFailure.into())
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
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let shown = Arc::new(Shown {
            digest,
            provider: Arc::clone(&provider),
        });
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(shown)
            .with_no_client_auth();
        Ok(Self(Arc::new(config)))
    }
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
