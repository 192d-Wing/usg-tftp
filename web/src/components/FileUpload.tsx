import { useCallback, useRef } from "react";
import Modal from "@cloudscape-design/components/modal";
import Button from "@cloudscape-design/components/button";
import SpaceBetween from "@cloudscape-design/components/space-between";
import Box from "@cloudscape-design/components/box";
import ProgressBar from "@cloudscape-design/components/progress-bar";
import StatusIndicator from "@cloudscape-design/components/status-indicator";
import Alert from "@cloudscape-design/components/alert";
import { useUpload } from "../hooks/useUpload";

interface FileUploadProps {
  visible: boolean;
  currentPath: string;
  onDismiss: () => void;
  onComplete: () => void;
}

interface FileWithPath {
  file: File;
  relativePath: string;
}

function collectEntries(entry: FileSystemEntry, basePath = ""): Promise<FileWithPath[]> {
  return new Promise((resolve, reject) => {
    if (entry.isFile) {
      (entry as FileSystemFileEntry).file(
        (f) => {
          const relativePath = basePath ? `${basePath}/${f.name}` : f.name;
          resolve([{ file: f, relativePath }]);
        },
        (err) => reject(err),
      );
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      const results: FileWithPath[] = [];
      const dirPath = basePath ? `${basePath}/${entry.name}` : entry.name;
      const readBatch = () => {
        reader.readEntries(
          async (entries) => {
            try {
              if (entries.length === 0) {
                resolve(results);
                return;
              }
              for (const e of entries) {
                const nested = await collectEntries(e, dirPath);
                results.push(...nested);
              }
              readBatch();
            } catch (err) {
              reject(err);
            }
          },
          (err) => reject(err),
        );
      };
      readBatch();
    } else {
      resolve([]);
    }
  });
}

export default function FileUpload({
  visible,
  currentPath,
  onDismiss,
  onComplete,
}: FileUploadProps) {
  const { uploading, progress, total, result, error, upload, reset } =
    useUpload();
  const dropRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleFilesWithPaths = useCallback(
    async (items: FileWithPath[]) => {
      if (items.length === 0) return;
      try {
        await upload(items, currentPath);
        onComplete();
      } catch {
        // error is tracked in useUpload state
      }
    },
    [upload, currentPath, onComplete],
  );

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const items = e.dataTransfer.items;
      const entries: FileSystemEntry[] = [];
      const plainFiles: FileWithPath[] = [];

      for (let i = 0; i < items.length; i++) {
        const entry = items[i].webkitGetAsEntry?.();
        if (entry) {
          entries.push(entry);
        } else {
          const file = items[i].getAsFile();
          if (file) plainFiles.push({ file, relativePath: file.name });
        }
      }

      const allFiles: FileWithPath[] = [...plainFiles];
      for (const entry of entries) {
        const files = await collectEntries(entry);
        allFiles.push(...files);
      }

      handleFilesWithPaths(allFiles);
    },
    [handleFilesWithPaths],
  );

  const handleInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const fileList = e.target.files;
      if (fileList) {
        handleFilesWithPaths(
          Array.from(fileList).map((f) => ({
            file: f,
            relativePath: f.webkitRelativePath || f.name,
          })),
        );
      }
    },
    [handleFilesWithPaths],
  );

  const handleDismiss = useCallback(() => {
    reset();
    onDismiss();
  }, [reset, onDismiss]);

  return (
    <Modal
      visible={visible}
      onDismiss={handleDismiss}
      header="Upload files"
      footer={
        <Box float="right">
          <Button variant="link" onClick={handleDismiss}>
            {result || error ? "Close" : "Cancel"}
          </Button>
        </Box>
      }
    >
      <SpaceBetween size="l">
        {uploading && (
          <ProgressBar
            value={total > 0 ? (progress / total) * 100 : 0}
            label="Uploading files"
            description={`${progress} of ${total} files`}
          />
        )}

        {error && <Alert type="error">{error}</Alert>}

        {result && (
          <SpaceBetween size="s">
            {result.uploaded.length > 0 && (
              <StatusIndicator type="success">
                {result.uploaded.length} file(s) uploaded
              </StatusIndicator>
            )}
            {result.errors.length > 0 && (
              <Alert type="warning" header="Some uploads failed">
                <ul>
                  {result.errors.map((err, i) => (
                    <li key={i}>{err}</li>
                  ))}
                </ul>
              </Alert>
            )}
          </SpaceBetween>
        )}

        {!uploading && !result && !error && (
          <div
            ref={dropRef}
            onDrop={handleDrop}
            onDragOver={(e) => {
              e.preventDefault();
              e.stopPropagation();
            }}
            style={{
              border: "2px dashed var(--color-border-input-default, #aab7b8)",
              borderRadius: "8px",
              padding: "40px",
              textAlign: "center",
              cursor: "pointer",
            }}
            onClick={() => inputRef.current?.click()}
          >
            <SpaceBetween size="s">
              <Box variant="h3">Drag & drop files or folders here</Box>
              <Box variant="p" color="text-body-secondary">
                or click to browse
              </Box>
              <Box variant="small" color="text-body-secondary">
                Uploading to: /{currentPath || "(root)"}
              </Box>
            </SpaceBetween>
            <input
              ref={inputRef}
              type="file"
              multiple
              onChange={handleInputChange}
              style={{ display: "none" }}
            />
          </div>
        )}
      </SpaceBetween>
    </Modal>
  );
}
