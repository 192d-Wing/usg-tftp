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

function collectEntries(entry: FileSystemEntry): Promise<File[]> {
  return new Promise((resolve) => {
    if (entry.isFile) {
      (entry as FileSystemFileEntry).file((f) => resolve([f]));
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      const files: File[] = [];
      const readBatch = () => {
        reader.readEntries(async (entries) => {
          if (entries.length === 0) {
            resolve(files);
            return;
          }
          for (const e of entries) {
            const nested = await collectEntries(e);
            files.push(...nested);
          }
          readBatch();
        });
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

  const handleFiles = useCallback(
    async (files: File[]) => {
      if (files.length === 0) return;
      try {
        await upload(files, currentPath);
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
      const allFiles: File[] = [];

      for (let i = 0; i < items.length; i++) {
        const entry = items[i].webkitGetAsEntry?.();
        if (entry) {
          const files = await collectEntries(entry);
          allFiles.push(...files);
        } else {
          const file = items[i].getAsFile();
          if (file) allFiles.push(file);
        }
      }

      handleFiles(allFiles);
    },
    [handleFiles],
  );

  const handleInputChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const fileList = e.target.files;
      if (fileList) {
        handleFiles(Array.from(fileList));
      }
    },
    [handleFiles],
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

        {!uploading && !result && (
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
