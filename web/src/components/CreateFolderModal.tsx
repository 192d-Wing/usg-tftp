import { useState, useCallback } from "react";
import Modal from "@cloudscape-design/components/modal";
import Button from "@cloudscape-design/components/button";
import Box from "@cloudscape-design/components/box";
import SpaceBetween from "@cloudscape-design/components/space-between";
import FormField from "@cloudscape-design/components/form-field";
import Input from "@cloudscape-design/components/input";
import Alert from "@cloudscape-design/components/alert";
import { createDirectory } from "../api/client";

interface CreateFolderModalProps {
  visible: boolean;
  currentPath: string;
  onDismiss: () => void;
  onConfirm: () => void;
}

export default function CreateFolderModal({
  visible,
  currentPath,
  onDismiss,
  onConfirm,
}: CreateFolderModalProps) {
  const [folderName, setFolderName] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameError =
    folderName.trim().startsWith(".")
      ? "Folder names starting with '.' are hidden and won't appear in the listing"
      : folderName.includes("/") || folderName.includes("\\")
        ? "Folder name cannot contain path separators"
        : null;

  const handleCreate = useCallback(async () => {
    if (!folderName.trim() || nameError) return;
    setCreating(true);
    setError(null);
    const fullPath = currentPath
      ? `${currentPath}/${folderName.trim()}`
      : folderName.trim();
    try {
      await createDirectory(fullPath);
      setFolderName("");
      onConfirm();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to create folder");
    } finally {
      setCreating(false);
    }
  }, [folderName, currentPath, onConfirm]);

  const handleDismiss = useCallback(() => {
    setFolderName("");
    setError(null);
    onDismiss();
  }, [onDismiss]);

  return (
    <Modal
      visible={visible}
      onDismiss={handleDismiss}
      header="Create folder"
      footer={
        <Box float="right">
          <SpaceBetween direction="horizontal" size="xs">
            <Button variant="link" onClick={handleDismiss}>
              Cancel
            </Button>
            <Button
              variant="primary"
              loading={creating}
              disabled={!folderName.trim() || !!nameError}
              onClick={handleCreate}
            >
              Create
            </Button>
          </SpaceBetween>
        </Box>
      }
    >
      <SpaceBetween size="m">
        <FormField
          label="Folder name"
          description={`Will be created in /${currentPath || "(root)"}`}
          errorText={folderName.trim() ? nameError : undefined}
        >
          <Input
            value={folderName}
            onChange={({ detail }) => setFolderName(detail.value)}
            placeholder="my-folder"
            autoFocus
          />
        </FormField>
        {error && <Alert type="error">{error}</Alert>}
      </SpaceBetween>
    </Modal>
  );
}
