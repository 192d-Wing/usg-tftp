import { useState, useCallback, useRef } from "react";
import { uploadFiles } from "../api/client";
import type { UploadResult } from "../api/types";

export interface FileWithPath {
  file: File;
  relativePath: string;
}

interface UploadState {
  uploading: boolean;
  progress: number;
  total: number;
  result: UploadResult | null;
  error: string | null;
}

export function useUpload() {
  const [state, setState] = useState<UploadState>({
    uploading: false,
    progress: 0,
    total: 0,
    result: null,
    error: null,
  });
  const abortRef = useRef<AbortController | null>(null);

  const upload = useCallback(async (items: FileWithPath[], targetPath: string) => {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    setState({
      uploading: true,
      progress: 0,
      total: items.length,
      result: null,
      error: null,
    });

    try {
      const result = await uploadFiles(
        items,
        targetPath,
        (uploaded, total) => {
          setState((prev) => ({ ...prev, progress: uploaded, total }));
        },
        controller.signal,
      );
      setState((prev) => ({ ...prev, uploading: false, result }));
      return result;
    } catch (e) {
      if (controller.signal.aborted) return { uploaded: [], errors: [] };
      const msg = e instanceof Error ? e.message : "Upload failed";
      setState((prev) => ({ ...prev, uploading: false, error: msg }));
      throw e;
    }
  }, []);

  const reset = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setState({
      uploading: false,
      progress: 0,
      total: 0,
      result: null,
      error: null,
    });
  }, []);

  return { ...state, upload, reset };
}
