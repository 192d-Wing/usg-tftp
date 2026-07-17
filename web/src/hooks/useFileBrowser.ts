import { useState, useEffect, useCallback } from "react";
import { listFiles } from "../api/client";
import type { FileEntry } from "../api/types";

export function useFileBrowser() {
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchFiles = useCallback(async (path: string) => {
    setLoading(true);
    setError(null);
    try {
      const entries = await listFiles(path);
      setFiles(entries);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load files");
      setFiles([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchFiles(currentPath);
  }, [currentPath, fetchFiles]);

  const navigate = useCallback((path: string) => {
    setCurrentPath(path);
  }, []);

  const refresh = useCallback(() => {
    fetchFiles(currentPath);
  }, [currentPath, fetchFiles]);

  return { files, currentPath, loading, error, navigate, refresh };
}
