use std::path::{Path, PathBuf};

use crate::error::{Result, TftpError};

pub fn validate_and_resolve_path(root_dir: &Path, filename: &str) -> Result<PathBuf> {
    let filename = filename.replace('\\', "/");
    if filename.contains("..") {
        return Err(TftpError::Tftp("Invalid filename".to_string()));
    }

    let file_path = root_dir.join(filename.trim_start_matches('/'));

    match std::fs::symlink_metadata(&file_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(TftpError::Tftp("Symlinks are not allowed".to_string()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(TftpError::Tftp("Access denied".to_string()));
        }
    }

    let canonical_root = root_dir
        .canonicalize()
        .map_err(|_| TftpError::Tftp("Root directory error".to_string()))?;

    if let Ok(canonical_file) = file_path.canonicalize() {
        if !canonical_file.starts_with(&canonical_root) {
            return Err(TftpError::Tftp("Access denied".to_string()));
        }
    } else if let Some(parent) = file_path.parent()
        && let Ok(canonical_parent) = parent.canonicalize()
        && !canonical_parent.starts_with(&canonical_root)
    {
        return Err(TftpError::Tftp("Access denied".to_string()));
    }

    Ok(file_path)
}
