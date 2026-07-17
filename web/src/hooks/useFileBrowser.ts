import { useState, useEffect, useCallback, useRef } from "react";
import { listFiles } from "../api/client";
import type { FileEntry } from "../api/types";

export function useFileBrowser() {
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const currentPathRef = useRef(currentPath);
  currentPathRef.current = currentPath;

  const fetchFiles = useCallback(async (path: string) => {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    setLoading(true);
    setError(null);
    try {
      const entries = await listFiles(path, controller.signal);
      if (!controller.signal.aborted) {
        setFiles(entries);
      }
    } catch (e) {
      if (controller.signal.aborted) return;
      setError(e instanceof Error ? e.message : "Failed to load files");
      setFiles([]);
    } finally {
      if (!controller.signal.aborted) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    fetchFiles(currentPath);
    return () => abortRef.current?.abort();
  }, [currentPath, fetchFiles]);

  const navigate = useCallback((path: string) => {
    setCurrentPath(path);
  }, []);

  const refresh = useCallback(() => {
    fetchFiles(currentPathRef.current);
  }, [fetchFiles]);

  return { files, currentPath, loading, error, navigate, refresh };
}
