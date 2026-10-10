//! End-to-end: issue a leaf from our CA, serve TLS with rustls, connect with a client that
//! trusts only our CA, complete the handshake and exchange bytes.

use bz_net::NetService;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn acceptor(cert: &std::path::Path, key: &std::path::Path) -> tokio_rustls::TlsAcceptor {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    let key = PrivateKeyDer::from_pem_file(key).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    tokio_rustls::TlsAcceptor::from(Arc::new(cfg))
}

fn connector(ca_pem: &std::path::Path) -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_file_iter(ca_pem).unwrap() {
        roots.add(c.unwrap()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
}

#[tokio::test]
async fn handshake_with_only_our_ca() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = NetService::with_dirs(
        tmp.path().join("ca"),
        tmp.path().join("certs"),
        tmp.path().join("backups"),
    );
    let (cert, key) = svc
        .ensure_cert("shop.test", &["admin.shop.test".into()], true)
        .unwrap();
    let ca = svc.ensure_ca().unwrap();
    let acc = acceptor(&cert, &key);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (s, _) = listener.accept().await.unwrap();
            let acc = acc.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acc.accept(s).await {
                    let mut buf = [0u8; 4];
                    if tls.read_exact(&mut buf).await.is_ok() {
                        let _ = tls.write_all(b"pong").await;
                        let _ = tls.shutdown().await;
                    }
                }
            });
        }
    });

    let conn = connector(&ca.cert_path);
    for host in ["shop.test", "api.shop.test", "admin.shop.test"] {
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut tls = conn
            .connect(ServerName::try_from(host.to_string()).unwrap(), tcp)
            .await
            .unwrap_or_else(|e| panic!("handshake for {host} failed: {e}"));
        tls.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
    }
    // A name not in the SANs must fail verification.
    let tcp = TcpStream::connect(addr).await.unwrap();
    let r = conn
        .connect(ServerName::try_from("other.test".to_string()).unwrap(), tcp)
        .await;
    assert!(r.is_err(), "wrong host name must be rejected");
    server.await.unwrap();
}

#[tokio::test]
async fn client_without_our_ca_rejects() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = NetService::with_dirs(
        tmp.path().join("ca"),
        tmp.path().join("certs"),
        tmp.path().join("backups"),
    );
    let (cert, key) = svc.ensure_cert("blog.test", &[], false).unwrap();
    let acc = acceptor(&cert, &key);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let _ = acc.accept(s).await;
    });
    // A different CA.
    let other = bz_net::LocalCa::new(tmp.path().join("other-ca"));
    let oi = other.generate_named("Some Other CA").unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    assert!(connector(&oi.cert_path)
        .connect(ServerName::try_from("blog.test".to_string()).unwrap(), tcp)
        .await
        .is_err());
}
