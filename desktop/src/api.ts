import { invoke } from "@tauri-apps/api/core";
export type Row = { id: string; [key: string]: any };
export type Job = {
  id: string;
  kind: string;
  status: string;
  message: string;
  started: number;
  finished?: number;
  historical?: boolean;
  result?: any;
};
export type AppState = {
  revision: number;
  roots: Row[];
  stats: Record<string, number>;
  job: Job | null;
  logs: { at: string; message: string; level?: string; category?: string; progress_id?: string }[];
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
        : Object.entries(value).map(([key, item]) => `${key.replaceAll("_", " ")}: ${readable(item)}`).join(" · ")
      : String(value);

export const tursoPing = () => call("turso.ping");
export const tursoStats = (market?: string, root?: string) =>
  call("turso.stats", { market, root });
export const tursoRoots = (market?: string) => call("turso.roots", { market });
export const tursoFiles = (root?: string, limit?: number, offset?: number) =>
  call("turso.files", { root, limit, offset });
export const tursoLinks = (args: Record<string, any> = {}) =>
  call("turso.links", args);
export const tursoMissing = (args: Record<string, any> = {}) =>
  call("turso.missing", args);
export const tursoScan = (root?: string) =>
  call("turso.scan", { args: { root } });
export const tursoTidalPing = () => call("turso.tidal.ping");
export const tursoTidalSearch = (query: string, market?: string) =>
  call("turso.tidal.search", { query, market });
export const tursoTidalArtist = (
  id: string,
  market?: string,
  detailed?: boolean,
) => call("turso.tidal.artist", { id, market, detailed });
export const tursoTagsWrite = (path: string, tags: Record<string, string>) =>
  call("turso.tags.write", { path, tags });
export const tursoOrganisationPreview = (
  root: string,
  tags: Record<string, string>,
  template?: string,
  extension?: string,
) => call("turso.organisation.preview", { root, tags, template, extension });
export const tursoKeysCanonical = (key: string) =>
  call("turso.keys.canonical", { key });
export const tursoDiscography = (args: {
  ids?: string[];
  detailed?: boolean;
  market?: string;
} = {}) => call("turso.discography", args);
export const tursoMaintenanceApply = (
  root: string,
  items: Array<{
    path: string;
    target?: string;
    tags?: Record<string, string>;
  }>,
) => call("turso.maintenance.apply", { root, items });
export const tursoMqaAudit = (path: string) =>
  call("turso.mqa.audit", { path });
export const tursoEnrichmentMissing = (args: {
  local_tags: Record<string, string>;
  release: any;
  track: any;
}) => call("turso.enrichment.missing", args);
export const tursoLink = (root?: string, market?: string) =>
  call("turso.link", { args: { root, market } });
export const tursoWorkflowsPlan = (
  root: string,
  action: string,
  template?: string,
) => call("turso.workflows.plan", { root, action, template });
export const tursoAccountStatus = () => call("turso.account.status");
export const tursoMatchingScore = (args: {
  local_name: string;
  local_albums: string[];
  candidate_id: string;
  candidate_name: string;
  candidate_albums: string[];
}) => call("turso.matching.score", args);


