use std::path::{Path, PathBuf};

use crate::config::WriteConfig;
use crate::error::{Result, TftpError};

pub fn is_write_allowed(file_path: &Path, root_dir: &Path, write_config: &WriteConfig) -> bool {
    let relative_path = match file_path.strip_prefix(root_dir) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let path_str = match relative_path.to_str() {
        Some(s) => s,
        None => return false,
    };

    for pattern in &write_config.allowed_patterns {
        if let Ok(glob_pattern) = glob::Pattern::new(pattern)
            && glob_pattern.matches(path_str)
        {
            return true;
        }
    }

    false
}

pub fn validate_and_resolve_path(root_dir: &Path, filename: &str) -> Result<PathBuf> {
    let filename = filename.replace('\\', "/");
    if filename
        .split('/')
        .any(|seg| seg == ".." || seg.starts_with('.'))
    {
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
    } else {
        let mut ancestor = file_path.parent();
        let mut found = false;
        while let Some(dir) = ancestor {
            if let Ok(canonical_dir) = dir.canonicalize() {
                if !canonical_dir.starts_with(&canonical_root) {
                    return Err(TftpError::Tftp("Access denied".to_string()));
                }
                found = true;
                break;
            }
            ancestor = dir.parent();
        }
        if !found {
            return Err(TftpError::Tftp("Access denied".to_string()));
        }
    }

    Ok(file_path)
}
