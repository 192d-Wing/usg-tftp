import { useState, useEffect } from "react";
import Container from "@cloudscape-design/components/container";
import Header from "@cloudscape-design/components/header";
import ColumnLayout from "@cloudscape-design/components/column-layout";
import Box from "@cloudscape-design/components/box";
import StatusIndicator from "@cloudscape-design/components/status-indicator";
import { getServerStatus } from "../api/client";
import type { ServerStatus } from "../api/types";

function formatUptime(seconds: number): string {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const parts = [];
  if (d > 0) parts.push(`${d}d`);
  if (h > 0) parts.push(`${h}h`);
  parts.push(`${m}m`);
  return parts.join(" ");
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`;
}

export default function ServerInfo() {
  const [status, setStatus] = useState<ServerStatus | null>(null);

  useEffect(() => {
    getServerStatus().then(setStatus).catch(() => {});
    const interval = setInterval(() => {
      getServerStatus().then(setStatus).catch(() => {});
    }, 30000);
    return () => clearInterval(interval);
  }, []);

  if (!status) return null;

  return (
    <Container header={<Header variant="h2">Server Status</Header>}>
      <ColumnLayout columns={4} variant="text-grid">
        <div>
          <Box variant="awsui-key-label">Version</Box>
          <div>{status.version}</div>
        </div>
        <div>
          <Box variant="awsui-key-label">Uptime</Box>
          <div>{formatUptime(status.uptime_seconds)}</div>
        </div>
        <div>
          <Box variant="awsui-key-label">TLS</Box>
          <StatusIndicator
            type={status.tls_mode === "none" ? "warning" : "success"}
          >
            {status.tls_mode}
          </StatusIndicator>
        </div>
        <div>
          <Box variant="awsui-key-label">Disk</Box>
          <div>
            {formatBytes(status.disk_available_bytes)} free /{" "}
            {formatBytes(status.disk_total_bytes)}
          </div>
        </div>
      </ColumnLayout>
    </Container>
  );
}
