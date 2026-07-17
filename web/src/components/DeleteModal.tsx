import { useState, useCallback, useEffect } from "react";
import Modal from "@cloudscape-design/components/modal";
import Button from "@cloudscape-design/components/button";
import Box from "@cloudscape-design/components/box";
import SpaceBetween from "@cloudscape-design/components/space-between";
import Alert from "@cloudscape-design/components/alert";
import { deleteFile } from "../api/client";
import type { FileEntry } from "../api/types";

interface DeleteModalProps {
  items: FileEntry[];
  onDismiss: () => void;
  onConfirm: () => void;
}

export default function DeleteModal({
  items,
  onDismiss,
  onConfirm,
}: DeleteModalProps) {
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
  }, [items]);

  const handleConfirm = useCallback(async () => {
    if (items.length === 0) return;
    setDeleting(true);
    setError(null);
    try {
      const errors: string[] = [];
      await Promise.all(
        items.map(async (item) => {
          try {
            await deleteFile(item.path);
          } catch (e) {
            errors.push(`${item.name}: ${e instanceof Error ? e.message : "failed"}`);
          }
        }),
      );
      if (errors.length > 0) {
        setError(errors.join("; "));
      } else {
        onConfirm();
      }
    } finally {
      setDeleting(false);
    }
  }, [items, onConfirm]);

  const hasDirs = items.some((i) => i.is_dir);

  return (
    <Modal
      visible={items.length > 0}
      onDismiss={deleting ? undefined : onDismiss}
      header={items.length === 1 ? `Delete ${items[0].is_dir ? "folder" : "file"}` : `Delete ${items.length} items`}
      footer={
        <Box float="right">
          <SpaceBetween direction="horizontal" size="xs">
            <Button variant="link" onClick={onDismiss} disabled={deleting}>
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
          {items.length === 1 ? (
            <>
              Are you sure you want to delete{" "}
              <strong>{items[0].name}</strong>?
              {items[0].is_dir && " This will delete all contents inside the folder."}
            </>
          ) : (
            <>
              Are you sure you want to delete these {items.length} items?
              {hasDirs && " Folders will have all their contents deleted."}
              <ul style={{ margin: "8px 0 0", paddingLeft: "20px" }}>
                {items.map((item) => (
                  <li key={item.path}>{item.name}</li>
                ))}
              </ul>
            </>
          )}
        </Box>
        {error && <Alert type="error">{error}</Alert>}
      </SpaceBetween>
    </Modal>
  );
}
