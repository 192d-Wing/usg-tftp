import { useState, useEffect } from "react";
import Table from "@cloudscape-design/components/table";
import Header from "@cloudscape-design/components/header";
import Button from "@cloudscape-design/components/button";
import SpaceBetween from "@cloudscape-design/components/space-between";
import BreadcrumbGroup from "@cloudscape-design/components/breadcrumb-group";
import Box from "@cloudscape-design/components/box";
import Icon from "@cloudscape-design/components/icon";
import Link from "@cloudscape-design/components/link";
import StatusIndicator from "@cloudscape-design/components/status-indicator";
import type { FileEntry } from "../api/types";
import { downloadFile } from "../api/client";
import FilePreview from "./FilePreview";

function formatSize(bytes: number, isDir: boolean): string {
  if (isDir) return "—";
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function formatDate(iso: string): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString();
}

interface FileBrowserProps {
  files: FileEntry[];
  currentPath: string;
  loading: boolean;
  error: string | null;
  onNavigate: (path: string) => void;
  onUploadClick: () => void;
  onDeleteClick: (files: FileEntry[]) => void;
  onCreateFolderClick: () => void;
  onRefresh: () => void;
}

export default function FileBrowser({
  files,
  currentPath,
  loading,
  error,
  onNavigate,
  onUploadClick,
  onDeleteClick,
  onCreateFolderClick,
  onRefresh,
}: FileBrowserProps) {
  const [selectedItems, setSelectedItems] = useState<FileEntry[]>([]);
  const [previewItem, setPreviewItem] = useState<FileEntry | null>(null);

  useEffect(() => {
    setSelectedItems([]);
  }, [files]);

  const breadcrumbs = [{ text: "Root", href: "" }];
  if (currentPath) {
    const parts = currentPath.split("/");
    let accumulated = "";
    for (const part of parts) {
      accumulated = accumulated ? `${accumulated}/${part}` : part;
      breadcrumbs.push({ text: part, href: accumulated });
    }
  }

  return (
    <SpaceBetween size="l">
      <BreadcrumbGroup
        items={breadcrumbs}
        onFollow={(e) => {
          e.preventDefault();
          onNavigate(e.detail.href);
        }}
      />
      <Table
        items={files}
        loading={loading}
        loadingText="Loading files..."
        selectionType="multi"
        selectedItems={selectedItems}
        onSelectionChange={({ detail }) =>
          setSelectedItems(detail.selectedItems)
        }
        onRowClick={({ detail }) => {
          const item = detail.item;
          if (item.is_dir) {
            onNavigate(item.path);
          } else {
            setPreviewItem(item);
          }
        }}
        empty={
          error ? (
            <StatusIndicator type="error">{error}</StatusIndicator>
          ) : (
            <Box textAlign="center" padding="l">
              <SpaceBetween size="m">
                <b>No files</b>
                <Box variant="p" color="text-body-secondary">
                  This directory is empty. Upload files to get started.
                </Box>
                <Button onClick={onUploadClick}>Upload files</Button>
              </SpaceBetween>
            </Box>
          )
        }
        header={
          <Header
            counter={`(${files.length})`}
            actions={
              <SpaceBetween direction="horizontal" size="xs">
                <Button iconName="refresh" onClick={onRefresh} />
                <Button onClick={onCreateFolderClick}>Create folder</Button>
                {selectedItems.length === 1 && !selectedItems[0].is_dir && (
                  <Button
                    onClick={() => downloadFile(selectedItems[0].path)}
                  >
                    Download
                  </Button>
                )}
                {selectedItems.length > 0 && (
                  <Button
                    onClick={() => onDeleteClick(selectedItems)}
                  >
                    Delete
                  </Button>
                )}
                <Button variant="primary" onClick={onUploadClick}>
                  Upload
                </Button>
              </SpaceBetween>
            }
          >
            Files
          </Header>
        }
        columnDefinitions={[
          {
            id: "name",
            header: "Name",
            cell: (item) => (
              <SpaceBetween direction="horizontal" size="xs">
                <Icon name={item.is_dir ? "folder" : "file"} />
                {item.is_dir ? (
                  <Link
                    onFollow={(e) => {
                      e.preventDefault();
                      onNavigate(item.path);
                    }}
                  >
                    {item.name}
                  </Link>
                ) : (
                  <span>{item.name}</span>
                )}
              </SpaceBetween>
            ),
            sortingField: "name",
            width: "40%",
          },
          {
            id: "size",
            header: "Size",
            cell: (item) => formatSize(item.size, item.is_dir),
            sortingField: "size",
            width: "15%",
          },
          {
            id: "modified",
            header: "Last modified",
            cell: (item) => formatDate(item.modified),
            sortingField: "modified",
            width: "25%",
          },
          {
            id: "type",
            header: "Type",
            cell: (item) =>
              item.is_dir
                ? "Folder"
                : item.name.includes(".")
                  ? item.name.split(".").pop()!.toUpperCase()
                  : "File",
            width: "20%",
          },
        ]}
      />
      {previewItem && (
        <FilePreview
          item={previewItem}
          onDismiss={() => setPreviewItem(null)}
          onDownload={() => downloadFile(previewItem.path)}
        />
      )}
    </SpaceBetween>
  );
}
