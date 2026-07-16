use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
}

#[derive(Debug, Serialize)]
pub struct ServerStatus {
    pub version: String,
    pub root_dir: String,
    pub uptime_seconds: u64,
    pub write_enabled: bool,
    pub disk_total_bytes: u64,
    pub disk_available_bytes: u64,
    pub tls_mode: String,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UploadResult {
    pub uploaded: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MkdirRequest {
    pub path: String,
}
