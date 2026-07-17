import { useState, useCallback } from "react";
import Layout from "./components/Layout";
import FileBrowser from "./components/FileBrowser";
import FileUpload from "./components/FileUpload";
import ServerInfo from "./components/ServerInfo";
import DeleteModal from "./components/DeleteModal";
import CreateFolderModal from "./components/CreateFolderModal";
import { useFileBrowser } from "./hooks/useFileBrowser";
import type { FileEntry } from "./api/types";

export default function App() {
  const { files, currentPath, loading, error, navigate, refresh } =
    useFileBrowser();
  const [showUpload, setShowUpload] = useState(false);
  const [deleteTargets, setDeleteTargets] = useState<FileEntry[]>([]);
  const [showCreateFolder, setShowCreateFolder] = useState(false);

  const handleUploadComplete = useCallback(() => {
    setShowUpload(false);
    refresh();
  }, [refresh]);

  const handleDeleteComplete = useCallback(() => {
    setDeleteTargets([]);
    refresh();
  }, [refresh]);

  const handleFolderCreated = useCallback(() => {
    setShowCreateFolder(false);
    refresh();
  }, [refresh]);

  return (
    <Layout>
      <ServerInfo />
      <FileBrowser
        files={files}
        currentPath={currentPath}
        loading={loading}
        error={error}
        onNavigate={navigate}
        onUploadClick={() => setShowUpload(true)}
        onDeleteClick={setDeleteTargets}
        onCreateFolderClick={() => setShowCreateFolder(true)}
        onRefresh={refresh}
      />
      <FileUpload
        visible={showUpload}
        currentPath={currentPath}
        onDismiss={() => setShowUpload(false)}
        onComplete={handleUploadComplete}
      />
      <DeleteModal
        items={deleteTargets}
        onDismiss={() => setDeleteTargets([])}
        onConfirm={handleDeleteComplete}
      />
      <CreateFolderModal
        visible={showCreateFolder}
        currentPath={currentPath}
        onDismiss={() => setShowCreateFolder(false)}
        onConfirm={handleFolderCreated}
      />
    </Layout>
  );
}
