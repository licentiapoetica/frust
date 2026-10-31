//! TCP / TLS connections for the IRC client and the Slack websocket.

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub type BoxStream = Box<dyn Stream>;

/// Accepts any certificate (still checks handshake signatures). Only used when a client is
/// configured with `selfSigned: true` (old `rejectUnauthorized: !selfSigned`).
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config(accept_invalid: bool) -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions()?;
    Ok(if accept_invalid {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider))).with_no_client_auth()
    } else {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        builder.with_root_certificates(roots).with_no_client_auth()
    })
}

/// Connect to `host:port`, optionally wrapped in TLS. Times out after 30 s.
pub async fn connect(host: &str, port: u16, tls: bool, accept_invalid_cert: bool) -> Result<BoxStream> {
    let tcp = tokio::time::timeout(Duration::from_secs(30), TcpStream::connect((host, port)))
        .await
        .with_context(|| format!("connect to {host}:{port} timed out"))?
        .with_context(|| format!("connect to {host}:{port}"))?;
    let _ = tcp.set_nodelay(true);
    if !tls {
        return Ok(Box::new(tcp));
    }
    let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config(accept_invalid_cert)?));
    let name = ServerName::try_from(host.to_string()).with_context(|| format!("invalid TLS server name {host}"))?;
    let stream = tokio::time::timeout(Duration::from_secs(30), connector.connect(name, tcp))
        .await
        .with_context(|| format!("TLS handshake with {host}:{port} timed out"))?
        .with_context(|| format!("TLS handshake with {host}:{port}"))?;
    Ok(Box::new(stream))
}
