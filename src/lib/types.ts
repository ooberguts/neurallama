// TypeScript mirrors of the Rust payload structs (serde camelCase).
// Source of truth: src-tauri/src/**/mod.rs, settings.rs, jobs.rs. Keep in sync.

// ---- settings.rs ----
export interface Paths {
  root: string;
  models: string;
  engines: string;
  logs: string;
  data: string;
}

export type KvType = "f16" | "q8_0" | "q4_0";

export interface BenchRecord {
  at: string;
  promptTps: number;
  genTps: number;
}

export interface Profile {
  id: string;
  name: string;
  modelPath: string;
  mmprojPath: string | null;
  ngl: number;
  nCpuMoe: number | null;
  cpuMoeAll: boolean;
  ctxSize: number;
  cacheTypeK: KvType | string;
  cacheTypeV: KvType | string;
  flashAttn: boolean;
  threads: number | null;
  threadsBatch: number | null;
  batch: number | null;
  ubatch: number | null;
  parallel: number;
  jinja: boolean;
  temp: number | null;
  topP: number | null;
  topK: number | null;
  minP: number | null;
  extraArgs: string;
  bench: BenchRecord | null;
}

export type EngineChannel = "stable" | "prerelease";

export interface Settings {
  host: string;
  port: number;
  apiKey: string;
  alias: string;
  lanMode: boolean;
  activeEngine: string | null;
  customServerExe: string | null;
  engineChannel: EngineChannel | string;
  activeProfile: string | null;
  profiles: Profile[];
  startWithWindows: boolean;
  autostartServer: boolean;
  tray: boolean;
  closeToTray: boolean;
  alwaysOnTop: boolean;
  autoRestart: boolean;
  dismissedTips: string[];
}

export interface AppInfo {
  version: string;
  os: string;
}

// ---- jobs.rs ----
export type JobState = "queued" | "running" | "paused" | "done" | "error" | "cancelled";
export type JobKind = "download" | "engine" | "autotune" | "benchmark";

export interface JobProgress {
  jobId: string;
  kind: JobKind | string;
  label: string;
  done: number;
  total: number | null;
  state: JobState;
  message: string | null;
}

// ---- server/mod.rs ----
export type ServerState = "stopped" | "starting" | "loading" | "ready" | "crashed" | "attached";

export interface ServerStatus {
  state: ServerState;
  pid: number | null;
  port: number;
  profileId: string | null;
  modelPath: string | null;
  startedAt: string | null;
  lastError: string | null;
  loadProgress: number | null;
  commandLine: string | null;
  crashCount: number;
}

export interface CommandPreview {
  program: string;
  args: string[];
  display: string;
}

// ---- engine/mod.rs ----
export type EngineFlavor = "cuda" | "vulkan" | "cpu" | "other";

export interface InstalledEngine {
  tag: string;
  flavor: EngineFlavor | string;
  dir: string;
  serverExe: string;
  active: boolean;
}

export interface EngineAsset {
  name: string;
  url: string;
  size: number;
  flavor: EngineFlavor | string;
  cudaVersion: string | null;
  cudartUrl: string | null;
  cudartSize: number | null;
}

export interface Release {
  tag: string;
  publishedAt: string;
  prerelease: boolean;
  htmlUrl: string;
  assets: EngineAsset[];
}

export interface DriverInfo {
  gpuName: string | null;
  driverVersion: string | null;
  cudaDriverVersion: string | null;
  computeCap: string | null;
  warnings: string[];
}

// ---- models/mod.rs ----
export interface GgufSummary {
  architecture: string;
  name: string | null;
  blockCount: number;
  contextLength: number | null;
  embeddingLength: number | null;
  headCount: number | null;
  headCountKv: number | null;
  keyLength: number | null;
  valueLength: number | null;
  expertCount: number | null;
  expertUsedCount: number | null;
  fileType: string | null;
  kvLayers: number | null;
  kvLayersSource: "exact" | "assumed" | string;
  recurrentStateBytes: number | null;
  isMoe: boolean;
  expertTensorBytes: number | null;
  totalTensorBytes: number | null;
}

export interface LocalModel {
  id: string;
  org: string | null;
  name: string;
  primaryPath: string;
  files: string[];
  sizeBytes: number;
  mmprojPath: string | null;
  quant: string | null;
  sourceKnown: boolean;
  repoId: string | null;
  meta: GgufSummary | null;
}

export interface HfModel {
  id: string;
  downloads: number;
  likes: number;
  lastModified: string | null;
  gated: boolean;
}

export interface HfFile {
  path: string;
  size: number;
  quant: string | null;
  isMmproj: boolean;
  shardGroup: string | null;
}

// ---- telemetry/mod.rs ----
export interface GpuStats {
  name: string;
  utilPct: number;
  vramUsed: number;
  vramTotal: number;
  tempC: number | null;
  powerW: number | null;
  clockMhz: number | null;
  source: "nvml" | "nvidia-smi" | string;
}

export interface MemStats {
  ramUsed: number;
  ramTotal: number;
  serverRss: number | null;
}

export interface ServerMetrics {
  promptTokensTotal: number | null;
  genTokensTotal: number | null;
  promptTps: number | null;
  genTps: number | null;
  requestsProcessing: number | null;
  requestsDeferred: number | null;
  kvUsage: number | null;
  slotsBusy: number | null;
  slotsTotal: number | null;
  ctxPerSlot: number | null;
  model: string | null;
  build: string | null;
}

export interface History {
  gpuUtil: number[];
  vramUsed: number[];
  genTps: number[];
}

export interface Snapshot {
  at: string;
  gpu: GpuStats | null;
  mem: MemStats;
  metrics: ServerMetrics | null;
  history: History;
}

export interface ModelStats {
  model: string;
  tokens: number;
  requests: number;
  avgTps: number;
  peakTps: number;
  lowTps: number;
}

export interface GpuProcess {
  pid: number;
  name: string;
  vramBytes: number | null;
}

// ---- fit/mod.rs ----
export interface FitInput {
  modelPath: string;
  ctxSize: number;
  cacheTypeK: string;
  cacheTypeV: string;
  ngl: number;
  nCpuMoe: number | null;
  cpuMoeAll: boolean;
  parallel: number;
  vramTotalOverride: number | null;
  /** -ub; affects the compute-buffer estimate. Optional (serde default). */
  ubatch?: number | null;
  /** -fa on; affects the compute-buffer estimate. Optional (serde default). */
  flashAttn?: boolean;
}

export type Accuracy = "measured" | "estimate" | "upper_bound";
export type Verdict = "fits" | "tight" | "wont_fit";
export type SuggestionKind =
  | "lower_ctx"
  | "kv_q8"
  | "kv_q4"
  | "more_cpu_moe"
  | "smaller_quant"
  | "close_apps";

export interface Suggestion {
  kind: SuggestionKind | string;
  label: string;
  /** Partial Profile (camelCase keys) to stage into the controls. */
  patch: Partial<Profile> & Record<string, unknown>;
}

export interface FitResult {
  weightsGpu: number;
  weightsCpu: number;
  kvBytes: number;
  kvAccuracy: Accuracy;
  recurrentBytes: number;
  computeOverhead: number;
  overheadAccuracy: Accuracy;
  displayReserve: number;
  vramNeeded: number;
  vramTotal: number;
  vramFreeNow: number | null;
  ramNeeded: number;
  ramTotal: number;
  verdict: Verdict;
  suggestions: Suggestion[];
  speedHint: string | null;
  measuredVram: number | null;
  errorPct: number | null;
  notes: string[];
}

export interface BenchResult {
  promptTokens: number;
  genTokens: number;
  promptTps: number;
  genTps: number;
  totalMs: number;
}

// ---- clients/mod.rs ----
export interface Snippets {
  curl: string;
  python: string;
  node: string;
}

export interface ConnectInfo {
  baseUrl: string;
  apiKey: string;
  modelId: string;
  lanUrls: string[];
  snippets: Snippets;
}

export interface TestResult {
  ok: boolean;
  status: number | null;
  message: string;
  latencyMs: number;
}

export interface HermesTarget {
  id: string;
  kind: "native" | "wsl" | string;
  distro: string | null;
  profile: string;
  isDefault: boolean;
  home: string;
  configPath: string;
  configExists: boolean;
  currentProvider: string | null;
  currentModel: string | null;
  pointsHere: boolean;
  baseUrl: string;
  note: string | null;
}

export interface HermesPushResult {
  targetId: string;
  ok: boolean;
  backupPath: string | null;
  message: string;
}

export interface ClientWriteResult {
  ok: boolean;
  path: string;
  backupPath: string | null;
  message: string;
}

// ---- events ----
export interface EventMap {
  "server-status": ServerStatus;
  "server-log": string[];
  telemetry: Snapshot;
  "job-progress": JobProgress;
}
