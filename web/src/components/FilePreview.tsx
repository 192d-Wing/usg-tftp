import Container from "@cloudscape-design/components/container";
import Header from "@cloudscape-design/components/header";
import Button from "@cloudscape-design/components/button";
import SpaceBetween from "@cloudscape-design/components/space-between";
import ColumnLayout from "@cloudscape-design/components/column-layout";
import Box from "@cloudscape-design/components/box";
import type { FileEntry } from "../api/types";

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

interface FilePreviewProps {
  item: FileEntry;
  onDismiss: () => void;
  onDownload: () => void;
}

export default function FilePreview({
  item,
  onDismiss,
  onDownload,
}: FilePreviewProps) {
  return (
    <Container
      header={
        <Header
          actions={
            <SpaceBetween direction="horizontal" size="xs">
              <Button onClick={onDownload}>Download</Button>
              <Button onClick={onDismiss}>Close</Button>
            </SpaceBetween>
          }
        >
          {item.name}
        </Header>
      }
    >
      <ColumnLayout columns={3}>
        <div>
          <Box variant="awsui-key-label">File name</Box>
          <div>{item.name}</div>
        </div>
        <div>
          <Box variant="awsui-key-label">Size</Box>
          <div>{formatSize(item.size)}</div>
        </div>
        <div>
          <Box variant="awsui-key-label">Last modified</Box>
          <div>{item.modified ? new Date(item.modified).toLocaleString() : "—"}</div>
        </div>
      </ColumnLayout>
    </Container>
  );
}
