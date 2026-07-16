use std::path::Path;

use axum::body::Body;
use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use tokio::fs;
use tokio_util::io::ReaderStream;
use tracing::{error, info};

use super::AppState;
use super::models::*;
use crate::path_security::validate_and_resolve_path;

fn api_error(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError { error: msg.into() })).into_response()
}

fn relative_path(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

pub async fn list_files(State(state): State<AppState>, Query(query): Query<FileQuery>) -> Response {
    let req_path = query.path.as_deref().unwrap_or("");
    let root = &state.config.root_dir;

    let dir_path = if req_path.is_empty() {
        root.clone()
    } else {
        match validate_and_resolve_path(root, req_path) {
            Ok(p) => p,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, e.to_string()),
        }
    };

    let mut read_dir = match fs::read_dir(&dir_path).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return api_error(StatusCode::NOT_FOUND, "Directory not found");
        }
        Err(e) => {
            error!("Failed to read directory: {}", e);
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list directory",
            );
        }
    };

    let mut entries = Vec::new();
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        entries.push(FileEntry {
            path: relative_path(root, &entry.path()),
            name,
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified,
        });
    }

    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    (StatusCode::OK, Json(entries)).into_response()
}

pub async fn download_file(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Response {
    let req_path = match query.path.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return api_error(StatusCode::BAD_REQUEST, "path is required"),
    };

    let file_path = match validate_and_resolve_path(&state.config.root_dir, req_path) {
        Ok(p) => p,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let file = match fs::File::open(&file_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return api_error(StatusCode::NOT_FOUND, "File not found");
        }
        Err(e) => {
            error!("Failed to open file: {}", e);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file");
        }
    };

    let filename = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let content_type = mime_guess::from_path(&file_path)
        .first_or_octet_stream()
        .to_string();

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(body)
        .unwrap_or_else(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Stream error"))
}

pub async fn upload_files(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
    mut multipart: Multipart,
) -> Response {
    let target_dir = query.path.as_deref().unwrap_or("");
    let root = &state.config.root_dir;

    let mut uploaded = Vec::new();
    let mut errors = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let relative = field.file_name().map(|s| s.to_string()).unwrap_or_default();
        if relative.is_empty() {
            continue;
        }

        let full_relative = if target_dir.is_empty() {
            relative.clone()
        } else {
            format!("{}/{}", target_dir.trim_end_matches('/'), relative)
        };

        let dest = match validate_and_resolve_path(root, &full_relative) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("{}: {}", relative, e));
                continue;
            }
        };

        if let Some(parent) = dest.parent()
            && let Err(e) = fs::create_dir_all(parent).await
        {
            errors.push(format!("{}: {}", relative, e));
            continue;
        }

        let data = match field.bytes().await {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("{}: {}", relative, e));
                continue;
            }
        };

        let tmp_path = dest.with_extension("tftp-tmp");
        if let Err(e) = fs::write(&tmp_path, &data).await {
            errors.push(format!("{}: {}", relative, e));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        if let Err(e) = fs::rename(&tmp_path, &dest).await {
            errors.push(format!("{}: {}", relative, e));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        info!(path = %full_relative, size = data.len(), "Web UI file uploaded");
        uploaded.push(full_relative);
    }

    let status = if errors.is_empty() {
        StatusCode::CREATED
    } else if uploaded.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::MULTI_STATUS
    };

    (status, Json(UploadResult { uploaded, errors })).into_response()
}

pub async fn delete_file(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    let req_path = match query.path.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return api_error(StatusCode::BAD_REQUEST, "path is required"),
    };

    let confirm = headers
        .get("X-Confirm")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if confirm != "true" {
        return api_error(StatusCode::BAD_REQUEST, "X-Confirm: true header required");
    }

    let file_path = match validate_and_resolve_path(&state.config.root_dir, req_path) {
        Ok(p) => p,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let meta = match fs::metadata(&file_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return api_error(StatusCode::NOT_FOUND, "Not found");
        }
        Err(e) => {
            error!("Failed to stat: {}", e);
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to access path");
        }
    };

    let result = if meta.is_dir() {
        fs::remove_dir_all(&file_path).await
    } else {
        fs::remove_file(&file_path).await
    };

    match result {
        Ok(()) => {
            info!(path = %req_path, is_dir = meta.is_dir(), "Web UI file deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            error!("Delete failed: {}", e);
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "Delete failed")
        }
    }
}

pub async fn create_directory(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Response {
    let req_path = match query.path.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => return api_error(StatusCode::BAD_REQUEST, "path is required"),
    };

    let dir_path = match validate_and_resolve_path(&state.config.root_dir, req_path) {
        Ok(p) => p,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    match fs::create_dir_all(&dir_path).await {
        Ok(()) => {
            info!(path = %req_path, "Web UI directory created");
            StatusCode::CREATED.into_response()
        }
        Err(e) => {
            error!("mkdir failed: {}", e);
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create directory",
            )
        }
    }
}

pub async fn server_status(State(state): State<AppState>) -> Json<ServerStatus> {
    let uptime = state.start_time.elapsed().as_secs();
    let tls_mode = if !state.config.web.tls.cert_path.is_empty() {
        "manual".to_string()
    } else if state.config.web.tls.acme_enabled {
        "acme".to_string()
    } else {
        "none".to_string()
    };

    #[cfg(target_os = "linux")]
    let (disk_total, disk_available) = {
        use std::ffi::CString;
        let path_c =
            CString::new(state.config.root_dir.to_string_lossy().as_bytes()).unwrap_or_default();
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_c.as_ptr(), &mut stat) == 0 {
                (
                    stat.f_blocks * stat.f_frsize as u64,
                    stat.f_bavail * stat.f_frsize as u64,
                )
            } else {
                (0, 0)
            }
        }
    };

    #[cfg(not(target_os = "linux"))]
    let (disk_total, disk_available) = (0u64, 0u64);

    Json(ServerStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        root_dir: state.config.root_dir.to_string_lossy().to_string(),
        uptime_seconds: uptime,
        write_enabled: state.config.write_config.enabled,
        disk_total_bytes: disk_total,
        disk_available_bytes: disk_available,
        tls_mode,
    })
}
