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

export interface AuditEntry {
  event_type: string;
  timestamp: string;
  path?: string;
  filename?: string;
  client_addr?: string;
  size?: number;
  is_dir?: boolean;
  source?: string;
  severity?: string;
  reason?: string;
  error?: string;
  bytes_transferred?: number;
  duration_ms?: number;
  [key: string]: unknown;
}

export interface AuditResponse {
  events: AuditEntry[];
  total: number;
  offset: number;
  limit: number;
}
