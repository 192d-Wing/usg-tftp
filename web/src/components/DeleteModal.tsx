import { useState, useCallback } from "react";
import Modal from "@cloudscape-design/components/modal";
import Button from "@cloudscape-design/components/button";
import Box from "@cloudscape-design/components/box";
import SpaceBetween from "@cloudscape-design/components/space-between";
import Alert from "@cloudscape-design/components/alert";
import { deleteFile } from "../api/client";
import type { FileEntry } from "../api/types";

interface DeleteModalProps {
  item: FileEntry | null;
  onDismiss: () => void;
  onConfirm: () => void;
}

export default function DeleteModal({
  item,
  onDismiss,
  onConfirm,
}: DeleteModalProps) {
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleConfirm = useCallback(async () => {
    if (!item) return;
    setDeleting(true);
    setError(null);
    try {
      await deleteFile(item.path);
      onConfirm();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Delete failed");
    } finally {
      setDeleting(false);
    }
  }, [item, onConfirm]);

  return (
    <Modal
      visible={!!item}
      onDismiss={onDismiss}
      header={`Delete ${item?.is_dir ? "folder" : "file"}`}
      footer={
        <Box float="right">
          <SpaceBetween direction="horizontal" size="xs">
            <Button variant="link" onClick={onDismiss}>
              Cancel
            </Button>
            <Button variant="primary" loading={deleting} onClick={handleConfirm}>
              Delete
            </Button>
          </SpaceBetween>
        </Box>
      }
    >
      <SpaceBetween size="m">
        <Box>
          Are you sure you want to delete{" "}
          <strong>{item?.name}</strong>?
          {item?.is_dir && " This will delete all contents inside the folder."}
        </Box>
        {error && <Alert type="error">{error}</Alert>}
      </SpaceBetween>
    </Modal>
  );
}
