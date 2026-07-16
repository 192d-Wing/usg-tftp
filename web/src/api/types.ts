export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: string;
}

export interface ServerStatus {
  version: string;
  root_dir: string;
  uptime_seconds: number;
  write_enabled: boolean;
  disk_total_bytes: number;
  disk_available_bytes: number;
  tls_mode: string;
}

export interface UploadResult {
  uploaded: string[];
  errors: string[];
}

export interface ApiError {
  error: string;
}
