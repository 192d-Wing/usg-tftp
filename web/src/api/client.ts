import type { FileEntry, ServerStatus, UploadResult, ApiError } from "./types";

const BASE = "";

async function handleResponse<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body: ApiError = await res.json().catch(() => ({
      error: `HTTP ${res.status}`,
    }));
    throw new Error(body.error);
  }
  return res.json();
}

export async function listFiles(path: string): Promise<FileEntry[]> {
  const params = path ? `?path=${encodeURIComponent(path)}` : "";
  const res = await fetch(`${BASE}/api/files${params}`);
  return handleResponse<FileEntry[]>(res);
}

export async function downloadFile(path: string): Promise<void> {
  const url = `${BASE}/api/files/download?path=${encodeURIComponent(path)}`;
  const a = document.createElement("a");
  a.href = url;
  a.download = path.split("/").pop() || "download";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export interface FileWithPath {
  file: File;
  relativePath: string;
}

export async function uploadFiles(
  items: FileWithPath[],
  targetPath: string,
  onProgress?: (uploaded: number, total: number) => void,
): Promise<UploadResult> {
  const allUploaded: string[] = [];
  const allErrors: string[] = [];
  const concurrency = 3;
  let completed = 0;

  const params = targetPath
    ? `?path=${encodeURIComponent(targetPath)}`
    : "";

  const sendOne = async (item: FileWithPath) => {
    const form = new FormData();
    form.append("file", item.file, item.relativePath);
    const res = await fetch(`${BASE}/api/files/upload${params}`, {
      method: "POST",
      body: form,
    });
    const result = await handleResponse<UploadResult>(res);
    allUploaded.push(...result.uploaded);
    allErrors.push(...result.errors);
    completed++;
    onProgress?.(completed, items.length);
  };

  for (let i = 0; i < items.length; i += concurrency) {
    const batch = items.slice(i, i + concurrency);
    await Promise.all(batch.map(sendOne));
  }

  return { uploaded: allUploaded, errors: allErrors };
}

export async function deleteFile(path: string): Promise<void> {
  const res = await fetch(
    `${BASE}/api/files?path=${encodeURIComponent(path)}`,
    {
      method: "DELETE",
      headers: { "X-Confirm": "true" },
    },
  );
  if (!res.ok) {
    const body: ApiError = await res.json().catch(() => ({
      error: `HTTP ${res.status}`,
    }));
    throw new Error(body.error);
  }
}

export async function createDirectory(path: string): Promise<void> {
  const res = await fetch(
    `${BASE}/api/files/mkdir?path=${encodeURIComponent(path)}`,
    { method: "POST" },
  );
  if (!res.ok) {
    const body: ApiError = await res.json().catch(() => ({
      error: `HTTP ${res.status}`,
    }));
    throw new Error(body.error);
  }
}

export async function getServerStatus(): Promise<ServerStatus> {
  const res = await fetch(`${BASE}/api/status`);
  return handleResponse<ServerStatus>(res);
}
