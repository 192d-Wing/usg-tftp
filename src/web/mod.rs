pub mod handlers;
pub mod models;
pub mod static_files;
pub mod tls;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::config::TftpConfig;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<TftpConfig>,
    pub start_time: std::time::Instant,
}

pub fn create_router(state: AppState) -> Router {
    let max_upload = state.config.web.max_upload_bytes as usize;
    let cors_enabled = state.config.web.cors_enabled;

    let upload = Router::new()
        .route("/api/files/upload", post(handlers::upload_files))
        .layer(DefaultBodyLimit::max(max_upload));

    let api = Router::new()
        .route("/api/files", get(handlers::list_files))
        .route("/api/files", delete(handlers::delete_file))
        .route("/api/files/download", get(handlers::download_file))
        .merge(upload)
        .route("/api/files/mkdir", post(handlers::create_directory))
        .route("/api/status", get(handlers::server_status));

    let mut app = Router::new()
        .merge(api)
        .fallback(static_files::serve_spa)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    if cors_enabled {
        app = app.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    app
}
