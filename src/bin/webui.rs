use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use usg_tftp::config::TftpConfig;
use usg_tftp::web;
use usg_tftp::web::audit::WebAuditLogger;

#[derive(Parser, Debug)]
#[command(name = "usg-tftp-webui", about = "USG TFTP Web UI")]
struct Cli {
    #[arg(short, long, default_value = "/etc/usg-tftp/tftp.toml")]
    config: PathBuf,

    #[arg(long)]
    bind: Option<SocketAddr>,

    #[arg(long)]
    root_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let cli = Cli::parse();

    let mut config = if cli.config.exists() {
        let contents = std::fs::read_to_string(&cli.config)?;
        toml::from_str::<TftpConfig>(&contents)?
    } else {
        info!("Config file not found, using defaults");
        TftpConfig::default()
    };

    if let Some(bind) = cli.bind {
        config.web.bind_addr = bind;
    }
    if let Some(root) = cli.root_dir {
        config.root_dir = root;
    }

    if !config.root_dir.exists() {
        std::fs::create_dir_all(&config.root_dir)?;
        info!(path = %config.root_dir.display(), "Created TFTP root directory");
    }

    let bind_addr = config.web.bind_addr;
    let tls_config = config.web.tls.clone();
    let audit_logger = WebAuditLogger::new(&config.web.audit_log_path);

    let state = web::AppState {
        config: Arc::new(config),
        start_time: std::time::Instant::now(),
        audit_logger,
    };

    let app = web::create_router(state);

    let proxy_protocol = config.web.proxy_protocol;
    info!(addr = %bind_addr, proxy_protocol, "Starting USG TFTP Web UI");
    web::tls::serve_https(app, bind_addr, &tls_config, proxy_protocol).await?;

    Ok(())
}
