// Typed wrappers for every Tauri command and event in docs/CONTRACT.md.
// In a plain browser (no Tauri runtime) calls go to an in-memory mock backend so
// `npm run dev` shows every tab populated.

import type {
  AppInfo,
  BenchResult,
  ClientWriteResult,
  CommandPreview,
  ConnectInfo,
  DriverInfo,
  EventMap,
  FitInput,
  FitResult,
  GgufSummary,
  GpuProcess,
  HermesPushResult,
  HermesTarget,
  HfFile,
  HfModel,
  InstalledEngine,
  JobProgress,
  LocalModel,
  ModelStats,
  Paths,
  Profile,
  Release,
  ServerStatus,
  Settings,
  Snapshot,
  TestResult,
} from "./types";

/** Every command: [args, return]. Arg names are the JS (camelCase) side. */
export interface Commands {
  // app / settings
  settings_get: [undefined, Settings];
  settings_save: [{ settings: Settings }, Settings];
  paths_get: [undefined, Paths];
  profile_upsert: [{ profile: Profile }, Settings];
  profile_delete: [{ id: string }, Settings];
  profile_set_active: [{ id: string }, Settings];
  api_key_regenerate: [undefined, Settings];
  app_info: [undefined, AppInfo];
  tip_dismiss: [{ id: string }, Settings];
  open_path: [{ path: string }, null];
  // server
  server_status: [undefined, ServerStatus];
  server_start: [{ profileId: string | null }, ServerStatus];
  server_stop: [undefined, ServerStatus];
  server_restart: [undefined, ServerStatus];
  server_log_tail: [{ lines: number }, string[]];
  server_command_preview: [{ profile: Profile }, CommandPreview];
  server_next_free_port: [undefined, number];
  // engine
  engine_installed: [undefined, InstalledEngine[]];
  engine_releases: [undefined, Release[]];
  engine_install: [{ tag: string; assetName: string }, string];
  engine_set_active: [{ tag: string }, Settings];
  engine_remove: [{ tag: string }, InstalledEngine[]];
  engine_driver_info: [{ cudaVersion: string | null } | undefined, DriverInfo];
  // models
  models_scan: [{ withMeta: boolean }, LocalModel[]];
  model_meta: [{ path: string }, GgufSummary];
  model_delete: [{ id: string }, LocalModel[]];
  model_serve: [{ id: string }, ServerStatus];
  hf_search: [{ query: string }, HfModel[]];
  hf_files: [{ repoId: string }, HfFile[]];
  download_start: [{ repoId: string; files: string[] }, string];
  download_pause: [{ jobId: string }, null];
  download_resume: [{ jobId: string }, null];
  download_cancel: [{ jobId: string }, null];
  downloads_list: [undefined, JobProgress[]];
  hf_token_set: [{ token: string | null }, null];
  hf_token_status: [undefined, boolean];
  disk_free: [undefined, number];
  // telemetry + fit
  telemetry_snapshot: [undefined, Snapshot];
  stats_list: [undefined, ModelStats[]];
  stats_reset: [{ model: string | null }, ModelStats[]];
  gpu_processes: [undefined, GpuProcess[]];
  fit_estimate: [{ input: FitInput }, FitResult];
  autotune_start: [{ profileId: string }, string];
  autotune_cancel: [{ jobId: string }, null];
  benchmark_run: [{ profileId: string | null }, BenchResult];
  // clients
  connect_info: [undefined, ConnectInfo];
  connect_test: [undefined, TestResult];
  hermes_targets: [undefined, HermesTarget[]];
  hermes_push: [{ targetId: string; contextLength: number | null }, HermesPushResult];
  continue_push: [undefined, ClientWriteResult];
}

export type CommandName = keyof Commands;
export type Args<K extends CommandName> = Commands[K][0];
export type Ret<K extends CommandName> = Commands[K][1];

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as object);

type MockModule = typeof import("./mock");
let mockPromise: Promise<MockModule> | null = null;
function mock(): Promise<MockModule> {
  mockPromise ??= import("./mock");
  return mockPromise;
}

/** Normalize whatever the backend rejected with into an Error carrying its plain-English text. */
function toError(e: unknown): Error {
  if (e instanceof Error) return e;
  if (typeof e === "string") return new Error(e);
  try {
    return new Error(JSON.stringify(e));
  } catch {
    return new Error(String(e));
  }
}

export async function call<K extends CommandName>(cmd: K, args?: Args<K>): Promise<Ret<K>> {
  try {
    if (isTauri) {
      const { invoke } = await import("@tauri-apps/api/core");
      return await invoke<Ret<K>>(cmd, (args ?? {}) as Record<string, unknown>);
    }
    const m = await mock();
    return (await m.invoke(cmd, args)) as Ret<K>;
  } catch (e) {
    throw toError(e);
  }
}

export type Unlisten = () => void;

export async function on<E extends keyof EventMap>(
  event: E,
  cb: (payload: EventMap[E]) => void,
): Promise<Unlisten> {
  if (isTauri) {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<EventMap[E]>(event, (e) => cb(e.payload));
  }
  const m = await mock();
  return m.listen(event, cb as (p: unknown) => void);
}

// ---- named wrappers (one per command) ----
export const api = {
  settingsGet: () => call("settings_get"),
  settingsSave: (settings: Settings) => call("settings_save", { settings }),
  pathsGet: () => call("paths_get"),
  profileUpsert: (profile: Profile) => call("profile_upsert", { profile }),
  profileDelete: (id: string) => call("profile_delete", { id }),
  profileSetActive: (id: string) => call("profile_set_active", { id }),
  apiKeyRegenerate: () => call("api_key_regenerate"),
  appInfo: () => call("app_info"),
  tipDismiss: (id: string) => call("tip_dismiss", { id }),
  openPath: (path: string) => call("open_path", { path }),

  serverStatus: () => call("server_status"),
  serverStart: (profileId: string | null = null) => call("server_start", { profileId }),
  serverStop: () => call("server_stop"),
  serverRestart: () => call("server_restart"),
  serverLogTail: (lines: number) => call("server_log_tail", { lines }),
  serverCommandPreview: (profile: Profile) => call("server_command_preview", { profile }),
  serverNextFreePort: () => call("server_next_free_port"),

  engineInstalled: () => call("engine_installed"),
  engineReleases: () => call("engine_releases"),
  engineInstall: (tag: string, assetName: string) => call("engine_install", { tag, assetName }),
  engineSetActive: (tag: string) => call("engine_set_active", { tag }),
  engineRemove: (tag: string) => call("engine_remove", { tag }),
  engineDriverInfo: (cudaVersion: string | null = null) => call("engine_driver_info", { cudaVersion }),

  modelsScan: (withMeta = true) => call("models_scan", { withMeta }),
  modelMeta: (path: string) => call("model_meta", { path }),
  modelDelete: (id: string) => call("model_delete", { id }),
  modelServe: (id: string) => call("model_serve", { id }),
  hfSearch: (query: string) => call("hf_search", { query }),
  hfFiles: (repoId: string) => call("hf_files", { repoId }),
  downloadStart: (repoId: string, files: string[]) => call("download_start", { repoId, files }),
  downloadPause: (jobId: string) => call("download_pause", { jobId }),
  downloadResume: (jobId: string) => call("download_resume", { jobId }),
  downloadCancel: (jobId: string) => call("download_cancel", { jobId }),
  downloadsList: () => call("downloads_list"),
  hfTokenSet: (token: string | null) => call("hf_token_set", { token }),
  hfTokenStatus: () => call("hf_token_status"),
  diskFree: () => call("disk_free"),

  telemetrySnapshot: () => call("telemetry_snapshot"),
  statsList: () => call("stats_list"),
  statsReset: (model: string | null = null) => call("stats_reset", { model }),
  gpuProcesses: () => call("gpu_processes"),
  fitEstimate: (input: FitInput) => call("fit_estimate", { input }),
  autotuneStart: (profileId: string) => call("autotune_start", { profileId }),
  autotuneCancel: (jobId: string) => call("autotune_cancel", { jobId }),
  benchmarkRun: (profileId: string | null = null) => call("benchmark_run", { profileId }),

  connectInfo: () => call("connect_info"),
  connectTest: () => call("connect_test"),
  hermesTargets: () => call("hermes_targets"),
  hermesPush: (targetId: string, contextLength: number | null = null) =>
    call("hermes_push", { targetId, contextLength }),
  continuePush: () => call("continue_push"),
};

export const events = {
  serverStatus: (cb: (s: ServerStatus) => void) => on("server-status", cb),
  serverLog: (cb: (lines: string[]) => void) => on("server-log", cb),
  telemetry: (cb: (s: Snapshot) => void) => on("telemetry", cb),
  jobProgress: (cb: (j: JobProgress) => void) => on("job-progress", cb),
};
