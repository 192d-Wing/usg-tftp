use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::ConnectInfo;
use tokio::net::TcpListener;
use tower::ServiceExt;
use tracing::info;

use crate::config::TlsConfig;

use super::proxy_protocol;

pub async fn serve_https(
    app: Router,
    bind_addr: SocketAddr,
    tls_config: &TlsConfig,
    proxy_protocol_enabled: bool,
) -> anyhow::Result<()> {
    if !tls_config.cert_path.is_empty() && !tls_config.key_path.is_empty() {
        serve_manual_tls(app, bind_addr, tls_config, proxy_protocol_enabled).await
    } else if tls_config.acme_enabled {
        serve_acme_tls(app, bind_addr, tls_config, proxy_protocol_enabled).await
    } else {
        info!("TLS disabled, serving HTTP on {}", bind_addr);
        let listener = TcpListener::bind(bind_addr).await?;
        if proxy_protocol_enabled {
            info!("PROXY protocol enabled (plain HTTP)");
            serve_with_proxy_protocol(app, listener, None).await
        } else {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;
            Ok(())
        }
    }
}

async fn serve_with_proxy_protocol(
    app: Router,
    listener: TcpListener,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
) -> anyhow::Result<()> {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder;

    loop {
        let (mut stream, peer_addr) = listener.accept().await?;
        let app = app.clone();
        let tls_acceptor = tls_acceptor.clone();

        tokio::spawn(async move {
            let real_addr = proxy_protocol::read_proxy_header(&mut stream)
                .await
                .unwrap_or(peer_addr);

            let serve = |io: TokioIo<_>| async move {
                let app = app.clone();
                let service = hyper::service::service_fn(move |mut req| {
                    req.extensions_mut().insert(ConnectInfo(real_addr));
                    let app = app.clone();
                    async move { app.oneshot(req).await }
                });
                let _ = Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(io, service)
                    .await;
            };

            if let Some(acceptor) = tls_acceptor {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => serve(TokioIo::new(tls_stream)).await,
                    Err(e) => tracing::debug!("TLS handshake failed: {}", e),
                }
            } else {
                serve(TokioIo::new(stream)).await;
            }
        });
    }
}

async fn serve_manual_tls(
    app: Router,
    bind_addr: SocketAddr,
    tls_config: &TlsConfig,
    proxy_protocol_enabled: bool,
) -> anyhow::Result<()> {
    info!("Starting HTTPS with manual certificates on {}", bind_addr);

    if proxy_protocol_enabled {
        let certs = load_certs(&tls_config.cert_path)?;
        let key = load_key(&tls_config.key_path)?;
        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

        let listener = TcpListener::bind(bind_addr).await?;
        info!("PROXY protocol enabled (manual TLS)");
        serve_with_proxy_protocol(app, listener, Some(tls_acceptor)).await
    } else {
        let rustls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            PathBuf::from(&tls_config.cert_path),
            PathBuf::from(&tls_config.key_path),
        )
        .await?;

        axum_server::bind_rustls(bind_addr, rustls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;

        Ok(())
    }
}

async fn serve_acme_tls(
    app: Router,
    bind_addr: SocketAddr,
    tls_config: &TlsConfig,
    proxy_protocol_enabled: bool,
) -> anyhow::Result<()> {
    use futures_lite::StreamExt;
    use rustls_acme::AcmeConfig;
    use rustls_acme::caches::DirCache;

    let domain = &tls_config.acme_domain;
    if domain.is_empty() {
        anyhow::bail!("ACME enabled but no domain configured (web.tls.acme_domain)");
    }

    info!(
        domain = %domain,
        "Starting HTTPS with ACME on {}",
        bind_addr
    );

    let cache_dir = if tls_config.acme_cache_dir.is_empty() {
        "/var/lib/usg-tftp/acme".to_string()
    } else {
        tls_config.acme_cache_dir.clone()
    };

    let mut acme_config = AcmeConfig::new([domain.as_str()])
        .cache(DirCache::new(cache_dir))
        .directory_lets_encrypt(!tls_config.acme_staging);

    if !tls_config.acme_ca_cert_path.is_empty() {
        let ca_pem = std::fs::read(&tls_config.acme_ca_cert_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read ACME CA cert {}: {}",
                tls_config.acme_ca_cert_path,
                e
            )
        })?;
        let mut root_store = rustls::RootCertStore::empty();
        let pem_certs: Vec<_> = rustls_pemfile::certs(&mut &ca_pem[..])
            .filter_map(|r| match r {
                Ok(cert) => Some(cert),
                Err(e) => {
                    tracing::warn!("Skipping invalid PEM certificate entry: {}", e);
                    None
                }
            })
            .collect();
        if pem_certs.is_empty() {
            let der_cert = rustls::pki_types::CertificateDer::from(ca_pem);
            root_store.add(der_cert)?;
        } else {
            for cert in pem_certs {
                root_store.add(cert)?;
            }
        }
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        acme_config = acme_config.client_tls_config(Arc::new(client_config));
        info!(
            "Loaded custom CA for ACME from {}",
            tls_config.acme_ca_cert_path
        );
    }

    if !tls_config.acme_email.is_empty() {
        acme_config = acme_config.contact([format!("mailto:{}", tls_config.acme_email)]);
    }

    if !tls_config.acme_directory_url.is_empty() {
        acme_config = acme_config.directory(tls_config.acme_directory_url.clone());
    }

    let mut acme_state = acme_config.state();

    if proxy_protocol_enabled {
        let tls_config = acme_state.default_rustls_config();
        let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_config);

        tokio::spawn(async move {
            while let Some(event) = acme_state.next().await {
                match event {
                    Ok(ok) => tracing::info!("ACME event: {:?}", ok),
                    Err(err) => tracing::error!("ACME error: {:?}", err),
                }
            }
        });

        let listener = TcpListener::bind(bind_addr).await?;
        info!("PROXY protocol enabled (ACME TLS)");
        serve_with_proxy_protocol(app, listener, Some(tls_acceptor)).await
    } else {
        let acceptor = acme_state.axum_acceptor(acme_state.default_rustls_config());

        tokio::spawn(async move {
            while let Some(event) = acme_state.next().await {
                match event {
                    Ok(ok) => tracing::info!("ACME event: {:?}", ok),
                    Err(err) => tracing::error!("ACME error: {:?}", err),
                }
            }
        });

        info!("ACME HTTPS listener ready on {}", bind_addr);

        axum_server::bind(bind_addr)
            .acceptor(acceptor)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;

        Ok(())
    }
}

fn load_certs(path: &str) -> anyhow::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let data = std::fs::read(path)?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut &data[..])
        .filter_map(|r| r.ok())
        .collect();
    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", path);
    }
    Ok(certs)
}

fn load_key(path: &str) -> anyhow::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let data = std::fs::read(path)?;
    rustls_pemfile::private_key(&mut &data[..])
        ?.ok_or_else(|| anyhow::anyhow!("No private key found in {}", path))
}
