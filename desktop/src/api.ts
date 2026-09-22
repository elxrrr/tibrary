import { invoke } from "@tauri-apps/api/core";
export type Row = { id: string; [key: string]: any };
export type Job = {
  id: string;
  kind: string;
  status: string;
  message: string;
  started: number;
  finished?: number;
  result?: any;
};
export type AppState = {
  recent_downloads?: Row[];
  revision: number;
  roots: Row[];
  stats: Record<string, number>;
  job: Job | null;
  logs: { at: string; message: string }[];
  settings: { market: string; theme: string };
  connections: any;
  diagnostics: any;
  demo: boolean;
  auth_url?: string;
};
export async function call<T = any>(
  method: string,
  args: Record<string, any> = {},
): Promise<T> {
  if (import.meta.env.VITE_TEST_BRIDGE === "1") {
    const response = await fetch("/__test_rpc", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ method, args }),
    });
    const value = await response.json();
    if (value.error) throw new Error(value.error);
    return value.result;
  }
  return invoke<T>("backend_call", { method, args });
}
export const active = (job?: Job | null) =>
  !!job && ["running", "cancelling"].includes(job.status);
export const external = (url: string) => invoke("open_external", { url });
export const reveal = (path: string) => invoke("reveal_file", { path });
export const readable = (value: any): string =>
  value == null
    ? "—"
    : typeof value === "object"
      ? Array.isArray(value)
        ? value.map(readable).join(" · ")
        : JSON.stringify(value)
      : String(value);
