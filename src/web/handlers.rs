use std::net::SocketAddr;
use std::path::Path;

use axum::body::Body;
use axum::extract::{ConnectInfo, Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use tokio::fs;
use tokio_util::io::ReaderStream;
use tracing::{error, info, warn};

use super::AppState;
use super::audit::{AuditQuery, AuditResponse};
use super::models::*;
use crate::path_security::validate_and_resolve_path;

fn client_ip(headers: &HeaderMap, conn: &ConnectInfo<SocketAddr>) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| conn.0.ip().to_string())
}

fn api_error(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError { error: msg.into() })).into_response()
}

#[allow(clippy::result_large_err)]
fn require_write(state: &AppState) -> Result<(), Response> {
    if !state.config.write_config.enabled {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "Write operations are disabled",
        ));
    }
    Ok(())
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
    loop {
        let entry = match read_dir.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(e) => {
                warn!("Error reading directory entry: {}", e);
                continue;
            }
        };
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to read metadata for {:?}: {}", entry.file_name(), e);
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "lost+found" {
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
    let safe_filename = filename.replace('"', "").replace(['\\', '\r', '\n'], "_");
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
            format!("attachment; filename=\"{}\"", safe_filename),
        )
        .body(body)
        .unwrap_or_else(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "Stream error"))
}

pub async fn upload_files(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
    mut multipart: Multipart,
) -> Response {
    if let Err(e) = require_write(&state) {
        return e;
    }
    let ip = client_ip(&headers, &ConnectInfo(addr));
    let target_dir = query.path.as_deref().unwrap_or("");
    let root = &state.config.root_dir;

    let mut uploaded = Vec::new();
    let mut errors = Vec::new();

    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                warn!("Error reading multipart field: {}", e);
                errors.push(format!("Multipart read error: {}", e));
                break;
            }
        };
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

        let tmp_name = format!(".tftp-tmp-{}", uuid::Uuid::new_v4());
        let tmp_path = dest.with_file_name(&tmp_name);
        let mut tmp_file = match fs::File::create(&tmp_path).await {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("{}: {}", relative, e));
                continue;
            }
        };

        let mut write_err = None;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut tmp_file, &chunk).await
                    {
                        write_err = Some(format!("{}", e));
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    write_err = Some(format!("Stream error: {}", e));
                    break;
                }
            }
        }
        drop(tmp_file);

        if let Some(err_msg) = write_err {
            errors.push(format!("{}: {}", relative, err_msg));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        if let Err(e) = fs::rename(&tmp_path, &dest).await {
            errors.push(format!("{}: {}", relative, e));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        let file_size = fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
        state
            .audit_logger
            .file_uploaded(&full_relative, file_size, &ip)
            .await;
        info!(path = %full_relative, "Web UI file uploaded");
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = require_write(&state) {
        return e;
    }
    let ip = client_ip(&headers, &ConnectInfo(addr));
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
            state
                .audit_logger
                .file_deleted(req_path, meta.is_dir(), &ip)
                .await;
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
) -> Response {
    if let Err(e) = require_write(&state) {
        return e;
    }
    let ip = client_ip(&headers, &ConnectInfo(addr));
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
            state.audit_logger.directory_created(req_path, &ip).await;
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

pub async fn audit_log(State(state): State<AppState>, Query(query): Query<AuditQuery>) -> Response {
    let log_path = state.audit_logger.log_path();
    let tftp_log_path = state.config.logging.file.as_deref();

    let mut all_events: Vec<serde_json::Value> = Vec::new();

    // Read web UI audit log
    if let Ok(contents) = tokio::fs::read_to_string(log_path).await {
        for line in contents.lines() {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
                all_events.push(event);
            }
        }
    }

    // Read TFTP audit log if accessible
    if let Some(tftp_path) = tftp_log_path
        && tftp_path.exists()
        && tftp_path != log_path
        && let Ok(contents) = tokio::fs::read_to_string(tftp_path).await
    {
        for line in contents.lines() {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
                all_events.push(event);
            }
        }
    }

    // Filter by event_type
    if let Some(ref event_type) = query.event_type {
        all_events.retain(|e| {
            e.get("event_type")
                .and_then(|v| v.as_str())
                .is_some_and(|t| t == event_type)
        });
    }

    // Filter by search (matches path/filename fields)
    if let Some(ref search) = query.search {
        let search_lower = search.to_lowercase();
        all_events.retain(|e| {
            let check_field = |field: &str| -> bool {
                e.get(field)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.to_lowercase().contains(&search_lower))
            };
            check_field("path") || check_field("filename") || check_field("client_addr")
        });
    }

    // Sort by timestamp descending (newest first)
    all_events.sort_by(|a, b| {
        let ts_a = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let ts_b = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        ts_b.cmp(ts_a)
    });

    let total = all_events.len();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).min(500);

    let events: Vec<serde_json::Value> = all_events.into_iter().skip(offset).take(limit).collect();

    (
        StatusCode::OK,
        Json(AuditResponse {
            events,
            total,
            offset,
            limit,
        }),
    )
        .into_response()
}
