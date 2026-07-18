use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum WebAuditEvent {
    WebFileUploaded {
        timestamp: String,
        path: String,
        size: u64,
        source: String,
    },
    WebFileDeleted {
        timestamp: String,
        path: String,
        is_dir: bool,
        source: String,
    },
    WebDirectoryCreated {
        timestamp: String,
        path: String,
        source: String,
    },
}

#[derive(Clone)]
pub struct WebAuditLogger {
    log_path: PathBuf,
}

impl WebAuditLogger {
    pub fn new(log_path: impl Into<PathBuf>) -> Self {
        Self {
            log_path: log_path.into(),
        }
    }

    async fn append(&self, event: &WebAuditEvent) {
        let line = match serde_json::to_string(event) {
            Ok(json) => format!("{}\n", json),
            Err(e) => {
                error!("Failed to serialize audit event: {}", e);
                return;
            }
        };

        if let Some(parent) = self.log_path.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            error!("Failed to create audit log directory: {}", e);
            return;
        }

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(line.as_bytes()).await {
                    error!("Failed to write audit event: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to open audit log: {}", e);
            }
        }
    }

    pub async fn file_uploaded(&self, path: &str, size: u64, client_ip: &str) {
        self.append(&WebAuditEvent::WebFileUploaded {
            timestamp: chrono::Utc::now().to_rfc3339(),
            path: path.to_string(),
            size,
            source: client_ip.to_string(),
        })
        .await;
    }

    pub async fn file_deleted(&self, path: &str, is_dir: bool, client_ip: &str) {
        self.append(&WebAuditEvent::WebFileDeleted {
            timestamp: chrono::Utc::now().to_rfc3339(),
            path: path.to_string(),
            is_dir,
            source: client_ip.to_string(),
        })
        .await;
    }

    pub async fn directory_created(&self, path: &str, client_ip: &str) {
        self.append(&WebAuditEvent::WebDirectoryCreated {
            timestamp: chrono::Utc::now().to_rfc3339(),
            path: path.to_string(),
            source: client_ip.to_string(),
        })
        .await;
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub event_type: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub events: Vec<serde_json::Value>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}
