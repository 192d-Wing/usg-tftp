use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use tokio::net::TcpListener;
use tower::ServiceExt;
use tracing::info;

use crate::config::TlsConfig;

use super::proxy_protocol;

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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

    let mut consecutive_errors = 0u32;
    loop {
        let (mut stream, _peer_addr) = match listener.accept().await {
            Ok(conn) => {
                consecutive_errors = 0;
                conn
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors >= 10 {
                    tracing::error!(
                        "Accept failing persistently ({}x): {}",
                        consecutive_errors,
                        e
                    );
                } else {
                    tracing::warn!("Accept error (continuing): {}", e);
                }
                let backoff = Duration::from_millis(50 * consecutive_errors.min(20) as u64);
                tokio::time::sleep(backoff).await;
                continue;
            }
        };
        let app = app.clone();
        let tls_acceptor = tls_acceptor.clone();

        tokio::spawn(async move {
            let real_addr = match proxy_protocol::read_proxy_header(&mut stream).await {
                Some(addr) => addr,
                None => {
                    tracing::debug!("PROXY header invalid or timed out, dropping connection");
                    return;
                }
            };

            let serve = |io: TokioIo<_>| async move {
                let service = hyper::service::service_fn(move |mut req| {
                    req.extensions_mut().insert(ConnectInfo(real_addr));
                    let app = app.clone();
                    async move { app.oneshot(req).await }
                });
                let mut builder = Builder::new(TokioExecutor::new());
                #[cfg(feature = "webui")]
                builder.http2().enable_connect_protocol();
                let _ = builder.serve_connection_with_upgrades(io, service).await;
            };

            if let Some(acceptor) = tls_acceptor {
                let tls_result =
                    tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await;
                match tls_result {
                    Ok(Ok(tls_stream)) => {
                        if tls_stream
                            .get_ref()
                            .1
                            .alpn_protocol()
                            .is_some_and(|p| p == b"acme-tls/1")
                        {
                            return;
                        }
                        serve(TokioIo::new(tls_stream)).await;
                    }
                    Ok(Err(e)) => tracing::debug!("TLS handshake failed: {}", e),
                    Err(_) => tracing::debug!("TLS handshake timed out"),
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
        let rustls_config = acme_state.default_rustls_config();

        // The cert resolver from default_rustls_config() handles both normal certs
        // and TLS-ALPN-01 challenges. We add ALPN protocols so negotiation works.
        let mut server_config = (*rustls_config).clone();
        server_config.alpn_protocols =
            vec![b"h2".to_vec(), b"http/1.1".to_vec(), b"acme-tls/1".to_vec()];
        let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));

        spawn_acme_event_loop(acme_state);

        let listener = TcpListener::bind(bind_addr).await?;
        info!("PROXY protocol enabled (ACME TLS)");
        serve_with_proxy_protocol(app, listener, Some(tls_acceptor)).await
    } else {
        let acceptor = acme_state.axum_acceptor(acme_state.default_rustls_config());

        spawn_acme_event_loop(acme_state);

        info!("ACME HTTPS listener ready on {}", bind_addr);

        axum_server::bind(bind_addr)
            .acceptor(acceptor)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;

        Ok(())
    }
}

fn spawn_acme_event_loop<EC: 'static + std::fmt::Debug, EA: 'static + std::fmt::Debug>(
    mut acme_state: rustls_acme::AcmeState<EC, EA>,
) {
    use futures_lite::StreamExt;
    tokio::spawn(async move {
        while let Some(event) = acme_state.next().await {
            match event {
                Ok(ok) => tracing::info!("ACME event: {:?}", ok),
                Err(err) => tracing::error!("ACME error: {:?}", err),
            }
        }
    });
}

fn load_certs(path: &str) -> anyhow::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let data = std::fs::read(path)?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut &data[..])
        .filter_map(|r| match r {
            Ok(cert) => Some(cert),
            Err(e) => {
                tracing::warn!("Skipping invalid certificate entry in {}: {}", path, e);
                None
            }
        })
        .collect();
    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", path);
    }
    Ok(certs)
}

fn load_key(path: &str) -> anyhow::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let data = std::fs::read(path)?;
    rustls_pemfile::private_key(&mut &data[..])?
        .ok_or_else(|| anyhow::anyhow!("No private key found in {}", path))
}
