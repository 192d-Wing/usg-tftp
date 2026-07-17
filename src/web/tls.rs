use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::TlsConfig;

pub async fn serve_https(
    app: Router,
    bind_addr: SocketAddr,
    tls_config: &TlsConfig,
) -> anyhow::Result<()> {
    if !tls_config.cert_path.is_empty() && !tls_config.key_path.is_empty() {
        serve_manual_tls(app, bind_addr, tls_config).await
    } else if tls_config.acme_enabled {
        serve_acme_tls(app, bind_addr, tls_config).await
    } else {
        info!("TLS disabled, serving HTTP on {}", bind_addr);
        let listener = TcpListener::bind(bind_addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn serve_manual_tls(
    app: Router,
    bind_addr: SocketAddr,
    tls_config: &TlsConfig,
) -> anyhow::Result<()> {
    info!("Starting HTTPS with manual certificates on {}", bind_addr);

    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        PathBuf::from(&tls_config.cert_path),
        PathBuf::from(&tls_config.key_path),
    )
    .await?;

    axum_server::bind_rustls(bind_addr, rustls_config)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn serve_acme_tls(
    app: Router,
    bind_addr: SocketAddr,
    tls_config: &TlsConfig,
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
            .filter_map(|r| r.ok())
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
        acme_config = acme_config.client_tls_config(std::sync::Arc::new(client_config));
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
        .serve(app.into_make_service())
        .await?;

    Ok(())
}
