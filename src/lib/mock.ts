// In-memory fake backend for plain browsers (no Tauri). Realistic data for an
// RTX 5090 Laptop (24 GB) + 64 GB RAM box serving Qwen3.6-35B-A3B.
// Only loaded via dynamic import from api.ts when __TAURI_INTERNALS__ is missing.

import type { Args, CommandName, Ret } from "./api";
import type {
  EngineAsset,
  FitInput,
  FitResult,
  GgufSummary,
  HfFile,
  HermesTarget,
  InstalledEngine,
  JobProgress,
  LocalModel,
  ModelStats,
  Profile,
  Release,
  ServerStatus,
  Settings,
  Snapshot,
  Suggestion,
} from "./types";

const GiB = 1024 ** 3;
const MiB = 1024 ** 2;
const ROOT = "C:\\Users\\you\\AI";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const rid = () => Math.random().toString(16).slice(2, 14);
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v));

// ---------------- event bus ----------------
type Cb = (p: unknown) => void;
const listeners = new Map<string, Set<Cb>>();
export function listen(event: string, cb: Cb): () => void {
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event)!.add(cb);
  start();
  return () => listeners.get(event)?.delete(cb);
}
function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((cb) => cb(clone(payload)));
}

// ---------------- data ----------------
const M35 = `${ROOT}\\models\\unsloth\\Qwen3.6-35B-A3B-GGUF\\Qwen3.6-35B-A3B-UD-Q5_K_M.gguf`;
const M27 = `${ROOT}\\models\\unsloth\\Qwen3.8-27B-GGUF\\Qwen3.8-27B-UD-Q4_K_M.gguf`;
const MFLAT = `${ROOT}\\models\\gemma-3-4b-it-Q8_0.gguf`;

const meta35: GgufSummary = {
  architecture: "qwen35moe",
  name: "Qwen3.6-35B-A3B",
  blockCount: 40,
  contextLength: 262144,
  embeddingLength: 2048,
  headCount: 16,
  headCountKv: 2,
  keyLength: 256,
  valueLength: 256,
  expertCount: 256,
  expertUsedCount: 8,
  fileType: "Q5_K_M",
  kvLayers: 10,
  kvLayersSource: "exact",
  recurrentStateBytes: 63 * MiB,
  isMoe: true,
  expertTensorBytes: Math.round(21.5 * GiB),
  totalTensorBytes: Math.round(22.8 * GiB),
};
const meta27: GgufSummary = {
  architecture: "qwen35",
  name: "Qwen3.8-27B",
  blockCount: 64,
  contextLength: 262144,
  embeddingLength: 5120,
  headCount: 24,
  headCountKv: 4,
  keyLength: 256,
  valueLength: 256,
  expertCount: null,
  expertUsedCount: null,
  fileType: "Q4_K_M",
  kvLayers: 16,
  kvLayersSource: "exact",
  recurrentStateBytes: 147 * MiB,
  isMoe: false,
  expertTensorBytes: null,
  totalTensorBytes: Math.round(16.4 * GiB),
};
const metaFlat: GgufSummary = {
  architecture: "gemma3",
  name: "gemma-3-4b-it",
  blockCount: 34,
  contextLength: 131072,
  embeddingLength: 2560,
  headCount: 8,
  headCountKv: 4,
  keyLength: 256,
  valueLength: 256,
  expertCount: null,
  expertUsedCount: null,
  fileType: "Q8_0",
  kvLayers: 34,
  kvLayersSource: "assumed",
  recurrentStateBytes: null,
  isMoe: false,
  expertTensorBytes: null,
  totalTensorBytes: Math.round(3.84 * GiB),
};

let models: LocalModel[] = [
  {
    id: "unsloth/Qwen3.6-35B-A3B-GGUF/Qwen3.6-35B-A3B-UD-Q5_K_M.gguf",
    org: "unsloth",
    name: "Qwen3.6-35B-A3B-GGUF",
    primaryPath: M35,
    files: [M35],
    sizeBytes: Math.round(22.9 * GiB),
    mmprojPath: `${ROOT}\\models\\unsloth\\Qwen3.6-35B-A3B-GGUF\\mmproj-F16.gguf`,
    quant: "UD-Q5_K_M",
    sourceKnown: true,
    repoId: "unsloth/Qwen3.6-35B-A3B-GGUF",
    meta: meta35,
  },
  {
    id: "unsloth/Qwen3.8-27B-GGUF/Qwen3.8-27B-UD-Q4_K_M.gguf",
    org: "unsloth",
    name: "Qwen3.8-27B-GGUF",
    primaryPath: M27,
    files: [M27],
    sizeBytes: Math.round(16.5 * GiB),
    mmprojPath: null,
    quant: "UD-Q4_K_M",
    sourceKnown: true,
    repoId: "unsloth/Qwen3.8-27B-GGUF",
    meta: meta27,
  },
  {
    id: "gemma-3-4b-it-Q8_0.gguf",
    org: null,
    name: "gemma-3-4b-it-Q8_0",
    primaryPath: MFLAT,
    files: [MFLAT],
    sizeBytes: Math.round(3.85 * GiB),
    mmprojPath: null,
    quant: "Q8_0",
    sourceKnown: false,
    repoId: null,
    meta: metaFlat,
  },
];

function baseProfile(): Profile {
  return {
    id: rid(),
    name: "New profile",
    modelPath: "",
    mmprojPath: null,
    ngl: 99,
    nCpuMoe: null,
    cpuMoeAll: false,
    ctxSize: 32768,
    cacheTypeK: "f16",
    cacheTypeV: "f16",
    flashAttn: true,
    threads: null,
    threadsBatch: null,
    batch: null,
    ubatch: null,
    parallel: 1,
    jinja: true,
    temp: null,
    topP: null,
    topK: null,
    minP: null,
    extraArgs: "",
    bench: null,
  };
}

let settings: Settings = {
  host: "127.0.0.1",
  port: 8080,
  apiKey: "nl-3f9c2a7e1b4d48f0a6c5e2d1b7f8a9c0",
  alias: "local",
  lanMode: false,
  activeEngine: "b7012-cuda-13.1",
  customServerExe: null,
  engineChannel: "stable",
  activeProfile: "preset-qwen3.6-35b-a3b-q5km-128k",
  profiles: [
    {
      ...baseProfile(),
      id: "preset-qwen3.6-35b-a3b-q5km-128k",
      name: "Qwen3.6-35B-A3B Q5_K_M, 128K",
      modelPath: M35,
      mmprojPath: models[0].mmprojPath,
      ctxSize: 131072,
      nCpuMoe: 8,
      cacheTypeK: "q8_0",
      cacheTypeV: "q8_0",
      temp: 0.7,
      topP: 0.8,
      topK: 20,
      minP: 0,
      bench: { at: "2026-11-09T21:14:03", promptTps: 2310.4, genTps: 94.2 },
    },
    {
      ...baseProfile(),
      id: "preset-qwen3.8-27b-q4km-64k",
      name: "Qwen3.8-27B Q4_K_M, 64K",
      modelPath: M27,
      ctxSize: 65536,
      cacheTypeK: "q8_0",
      cacheTypeV: "q8_0",
      bench: { at: "2026-11-08T10:02:41", promptTps: 1880.2, genTps: 47.6 },
    },
  ],
  startWithWindows: true,
  autostartServer: true,
  tray: true,
  closeToTray: false,
  alwaysOnTop: false,
  autoRestart: true,
  dismissedTips: [],
};

const startedAt = new Date(Date.now() - 14 * 60_000).toISOString();
let status: ServerStatus = {
  state: "ready",
  pid: 18244,
  port: 8080,
  profileId: settings.activeProfile,
  modelPath: M35,
  startedAt,
  lastError: null,
  loadProgress: null,
  commandLine: null,
  crashCount: 0,
};

let engines: InstalledEngine[] = [
  {
    tag: "b7012-cuda-13.1",
    flavor: "cuda",
    dir: `${ROOT}\\llama.cpp\\b7012-cuda-13.1`,
    serverExe: `${ROOT}\\llama.cpp\\b7012-cuda-13.1\\llama-server.exe`,
    active: true,
  },
  {
    tag: "b6940-cuda-13.1",
    flavor: "cuda",
    dir: `${ROOT}\\llama.cpp\\b6940-cuda-13.1`,
    serverExe: `${ROOT}\\llama.cpp\\b6940-cuda-13.1\\llama-server.exe`,
    active: false,
  },
  {
    tag: "b6940-vulkan",
    flavor: "vulkan",
    dir: `${ROOT}\\llama.cpp\\b6940-vulkan`,
    serverExe: `${ROOT}\\llama.cpp\\b6940-vulkan\\llama-server.exe`,
    active: false,
  },
];

function assetsFor(tag: string): EngineAsset[] {
  const gh = `https://github.com/ggml-org/llama.cpp/releases/download/${tag}`;
  return [
    {
      name: `llama-${tag}-bin-win-cuda-12.4-x64.zip`,
      url: `${gh}/llama-${tag}-bin-win-cuda-12.4-x64.zip`,
      size: 214 * MiB,
      flavor: "cuda",
      cudaVersion: "12.4",
      cudartUrl: `${gh}/cudart-llama-bin-win-cuda-12.4-x64.zip`,
      cudartSize: 467 * MiB,
    },
    {
      name: `llama-${tag}-bin-win-cuda-13.1-x64.zip`,
      url: `${gh}/llama-${tag}-bin-win-cuda-13.1-x64.zip`,
      size: 198 * MiB,
      flavor: "cuda",
      cudaVersion: "13.1",
      cudartUrl: `${gh}/cudart-llama-bin-win-cuda-13.1-x64.zip`,
      cudartSize: 389 * MiB,
    },
    {
      name: `llama-${tag}-bin-win-vulkan-x64.zip`,
      url: `${gh}/llama-${tag}-bin-win-vulkan-x64.zip`,
      size: 41 * MiB,
      flavor: "vulkan",
      cudaVersion: null,
      cudartUrl: null,
      cudartSize: null,
    },
    {
      name: `llama-${tag}-bin-win-cpu-x64.zip`,
      url: `${gh}/llama-${tag}-bin-win-cpu-x64.zip`,
      size: 23 * MiB,
      flavor: "cpu",
      cudaVersion: null,
      cudartUrl: null,
      cudartSize: null,
    },
  ];
}
function releases(): Release[] {
  const list: Release[] = [
    { tag: "b7031", publishedAt: "2026-11-11T06:12:00Z", prerelease: false },
    { tag: "b7025", publishedAt: "2026-11-10T14:40:00Z", prerelease: false },
    { tag: "b7012", publishedAt: "2026-11-07T09:03:00Z", prerelease: false },
    { tag: "b6940", publishedAt: "2026-10-28T18:22:00Z", prerelease: false },
  ].map((r, i) => ({
    ...r,
    stableTag: settings.engineChannel === "stable" ? ["v1.31.0", "v1.30.2", "v1.30.0", "v1.29.1"][i] : null,
    htmlUrl: `https://github.com/ggml-org/llama.cpp/releases/tag/${r.tag}`,
    assets: assetsFor(r.tag),
  }));
  if (settings.engineChannel === "prerelease")
    list.unshift({
      tag: "b7034",
      publishedAt: "2026-11-11T15:50:00Z",
      prerelease: true,
      htmlUrl: "https://github.com/ggml-org/llama.cpp/releases/tag/b7034",
      assets: assetsFor("b7034"),
    });
  return list;
}

let stats: ModelStats[] = [
  { model: "Qwen3.6-35B-A3B-UD-Q5_K_M", tokens: 4_812_330, requests: 1_284, avgTps: 91.3, peakTps: 118.6, lowTps: 42.1 },
  { model: "Qwen3.8-27B-UD-Q4_K_M", tokens: 1_207_915, requests: 402, avgTps: 46.8, peakTps: 55.2, lowTps: 31.9 },
  { model: "gemma-3-4b-it-Q8_0", tokens: 88_410, requests: 57, avgTps: 182.5, peakTps: 214.0, lowTps: 120.4 },
];

let hfTokenStored = true;

const hermes: HermesTarget[] = [
  {
    id: "native:default",
    kind: "native",
    distro: null,
    profile: "default",
    isDefault: true,
    home: "C:\\Users\\you\\AppData\\Local\\hermes",
    configPath: "C:\\Users\\you\\AppData\\Local\\hermes\\config.yaml",
    configExists: true,
    currentProvider: "openrouter",
    currentModel: "qwen/qwen3-coder",
    pointsHere: false,
    baseUrl: "http://127.0.0.1:8080/v1",
    note: null,
  },
  {
    id: "native:local-dev",
    kind: "native",
    distro: null,
    profile: "local-dev",
    isDefault: false,
    home: "C:\\Users\\you\\AppData\\Local\\hermes\\profiles\\local-dev",
    configPath: "C:\\Users\\you\\AppData\\Local\\hermes\\profiles\\local-dev\\config.yaml",
    configExists: true,
    currentProvider: "neurallama",
    currentModel: "local",
    pointsHere: true,
    baseUrl: "http://127.0.0.1:8080/v1",
    note: null,
  },
  {
    id: "wsl:Ubuntu-24.04:default",
    kind: "wsl",
    distro: "Ubuntu-24.04",
    profile: "default",
    isDefault: true,
    home: "\\\\wsl$\\Ubuntu-24.04\\home\\you\\.hermes",
    configPath: "\\\\wsl$\\Ubuntu-24.04\\home\\you\\.hermes\\config.yaml",
    configExists: true,
    currentProvider: "anthropic",
    currentModel: null,
    pointsHere: false,
    baseUrl: "http://172.29.80.1:8080/v1",
    note: "WSL is in NAT mode, so 127.0.0.1 won't reach Windows. Turn on LAN mode, or switch WSL to mirrored networking.",
  },
];

// ---------------- jobs ----------------
const jobs = new Map<string, JobProgress & { _paused?: boolean; _cancel?: boolean }>();
function pushJob(j: JobProgress) {
  jobs.set(j.jobId, j);
  emit("job-progress", j);
}
function seedDownloads() {
  pushJob({
    jobId: "dl-seed-1",
    kind: "download",
    label: "unsloth/Qwen3.6-35B-A3B-GGUF · UD-Q4_K_XL",
    done: Math.round(9.2 * GiB),
    total: Math.round(21.4 * GiB),
    state: "running",
    message: "38.4 MB/s",
  });
  pushJob({
    jobId: "dl-seed-2",
    kind: "download",
    label: "unsloth/Qwen3.8-27B-GGUF · mmproj-F16.gguf",
    done: Math.round(0.31 * GiB),
    total: Math.round(0.88 * GiB),
    state: "paused",
    message: null,
  });
}

function tickJobs() {
  for (const j of jobs.values()) {
    if (j.kind !== "download" || j.state !== "running" || !j.total) continue;
    const rate = (30 + Math.random() * 15) * MiB;
    j.done = Math.min(j.total, j.done + rate);
    j.message = `${(rate / 1e6).toFixed(1)} MB/s`;
    if (j.done >= j.total) {
      j.state = "done";
      j.message = "Verified and saved";
    }
    emit("job-progress", j);
  }
}

async function runStepJob(kind: string, label: string, steps: string[], ms: number, onDone?: () => void) {
  const id = rid();
  const j: JobProgress & { _cancel?: boolean } = { jobId: id, kind, label, done: 0, total: steps.length, state: "running", message: steps[0] };
  pushJob(j);
  (async () => {
    for (let i = 0; i < steps.length; i++) {
      await sleep(ms);
      if (j._cancel) {
        j.state = "cancelled";
        j.message = "Cancelled";
        emit("job-progress", j);
        return;
      }
      j.done = i + 1;
      j.message = steps[i + 1] ?? steps[i];
      if (i === steps.length - 1) {
        j.state = "done";
        onDone?.();
      }
      emit("job-progress", j);
    }
  })();
  return id;
}

// ---------------- telemetry sim ----------------
const N = 90;
const hist = { gpuUtil: [] as number[], vramUsed: [] as number[], genTps: [] as number[] };
let phase = 0;
let burstLeft = 0;
let idleLeft = 4;
let genTotal = 912_448;
let promptTotal = 3_402_118;
let kv = 0.18;
function step() {
  const ready = status.state === "ready";
  let tps = 0;
  let util = 3 + Math.random() * 4;
  if (ready) {
    if (burstLeft > 0) {
      burstLeft--;
      tps = 88 + Math.sin(phase / 3) * 9 + Math.random() * 8;
      util = 78 + Math.random() * 18;
      kv = Math.min(0.92, kv + 0.006);
      genTotal += Math.round(tps);
      if (burstLeft === 0) idleLeft = 3 + Math.floor(Math.random() * 8);
    } else if (idleLeft > 0) {
      idleLeft--;
      if (idleLeft === 0) {
        burstLeft = 6 + Math.floor(Math.random() * 14);
        promptTotal += 1800 + Math.floor(Math.random() * 6000);
      }
    }
  }
  phase++;
  const vram = ready ? 21.4 * GiB + Math.random() * 0.15 * GiB : 1.3 * GiB;
  hist.gpuUtil.push(util);
  hist.vramUsed.push(vram);
  hist.genTps.push(tps);
  for (const a of Object.values(hist)) if (a.length > N) a.shift();
  return { tps, util, vram, busy: burstLeft > 0 };
}
for (let i = 0; i < N; i++) step();

function snapshot(): Snapshot {
  const last = hist.genTps.length - 1;
  const busy = hist.genTps[last] > 0;
  const ready = status.state === "ready";
  return {
    at: new Date().toISOString(),
    gpu: {
      name: "NVIDIA GeForce RTX 5090 Laptop GPU",
      utilPct: hist.gpuUtil[last],
      vramUsed: hist.vramUsed[last],
      vramTotal: 24 * GiB,
      tempC: busy ? 71 + Math.random() * 4 : 52 + Math.random() * 2,
      powerW: busy ? 128 + Math.random() * 20 : 18 + Math.random() * 4,
      clockMhz: busy ? 2385 : 765,
      source: "nvml",
    },
    mem: { ramUsed: Math.round((ready ? 38.6 : 23.1) * GiB + Math.random() * 0.2 * GiB), ramTotal: 64 * GiB, serverRss: ready ? Math.round(14.2 * GiB) : null },
    metrics: ready
      ? {
          promptTokensTotal: promptTotal,
          genTokensTotal: genTotal,
          promptTps: busy ? 2240 + Math.random() * 120 : null,
          genTps: busy ? hist.genTps[last] : 0,
          requestsProcessing: busy ? 1 : 0,
          requestsDeferred: busy && Math.random() > 0.7 ? 1 : 0,
          kvUsage: kv,
          slotsBusy: busy ? 1 : 0,
          slotsTotal: 1,
          ctxPerSlot: 131072,
          model: "Qwen3.6-35B-A3B-UD-Q5_K_M.gguf",
          build: "b7012-a4c1d93",
        }
      : null,
    history: clone(hist),
  };
}

// ---------------- logs ----------------
const logs: string[] = [];
let task = 4410;
function ts() {
  return new Date().toISOString().slice(11, 23);
}
function seedLogs() {
  const boot = [
    "build: 7012 (a4c1d93) with MSVC 19.44 for x64",
    "ggml_cuda_init: found 1 CUDA devices:",
    "  Device 0: NVIDIA GeForce RTX 5090 Laptop GPU, compute capability 12.0, VMM: yes",
    `llama_model_loader: loaded meta data with 52 key-value pairs and 733 tensors from ${M35}`,
    "load_tensors: offloading 40 repeating layers to GPU",
    "load_tensors: offloaded 41/41 layers to GPU",
    "load_tensors:        CUDA0 model buffer size = 18342.11 MiB",
    "load_tensors:   CPU_Mapped model buffer size =  6981.40 MiB",
    "llama_context: n_ctx = 131072, n_ctx_per_seq = 131072, n_batch = 2048, n_ubatch = 512, flash_attn = 1",
    "llama_kv_cache:      CUDA0 KV buffer size =  1360.00 MiB",
    "llama_memory_recurrent:      CUDA0 RS buffer size =    62.81 MiB",
    "llama_context:      CUDA0 compute buffer size =   986.00 MiB",
    "main: model loaded",
    "main: server is listening on http://127.0.0.1:8080 - starting the main loop",
    "srv  update_slots: all slots are idle",
  ];
  for (const l of boot) logs.push(`${ts()} ${l}`);
  for (let i = 0; i < 40; i++) logs.push(...requestLines());
}
function requestLines(): string[] {
  task++;
  const p = 900 + Math.floor(Math.random() * 5000);
  const g = 120 + Math.floor(Math.random() * 900);
  const ptps = 2100 + Math.random() * 300;
  const gtps = 82 + Math.random() * 20;
  const out = [
    `${ts()} slot launch_slot_: id  0 | task ${task} | processing task`,
    `${ts()} slot update_slots: id  0 | task ${task} | new prompt, n_ctx_slot = 131072, n_keep = 0, n_prompt_tokens = ${p}`,
    `${ts()} slot print_timing: id  0 | task ${task} | prompt eval time = ${((p / ptps) * 1000).toFixed(2)} ms / ${p} tokens (${(1000 / ptps).toFixed(2)} ms per token, ${ptps.toFixed(2)} tokens per second)`,
    `${ts()}        eval time = ${((g / gtps) * 1000).toFixed(2)} ms / ${g} tokens (${(1000 / gtps).toFixed(2)} ms per token, ${gtps.toFixed(2)} tokens per second)`,
    `${ts()} slot      release: id  0 | task ${task} | stop processing: n_past = ${p + g}, truncated = 0`,
    `${ts()} srv  log_server_r: request: POST /v1/chat/completions 127.0.0.1 200`,
  ];
  if (Math.random() > 0.85) out.push(`${ts()} srv  log_server_r: request: GET /metrics 127.0.0.1 200`);
  return out;
}
function pushLogs(lines: string[]) {
  logs.push(...lines);
  if (logs.length > 3000) logs.splice(0, logs.length - 3000);
  emit("server-log", lines);
}

// ---------------- timers ----------------
let started = false;
function start() {
  if (started) return;
  started = true;
  seedLogs();
  seedDownloads();
  setInterval(() => {
    const s = step();
    emit("telemetry", snapshot());
    tickJobs();
    if (status.state === "ready" && s.busy && Math.random() > 0.8) pushLogs(requestLines());
  }, 1000);
}

function setStatus(patch: Partial<ServerStatus>) {
  status = { ...status, ...patch };
  emit("server-status", status);
}

async function bootServer(profileId: string | null) {
  const p = settings.profiles.find((x) => x.id === (profileId ?? settings.activeProfile));
  setStatus({
    state: "starting",
    pid: 20000 + Math.floor(Math.random() * 9999),
    profileId: p?.id ?? null,
    modelPath: p?.modelPath ?? null,
    lastError: null,
    loadProgress: null,
    startedAt: new Date().toISOString(),
  });
  pushLogs([`${ts()} build: 7012 (a4c1d93) with MSVC 19.44 for x64`]);
  (async () => {
    await sleep(500);
    for (let i = 1; i <= 5; i++) {
      setStatus({ state: "loading", loadProgress: i / 5 });
      pushLogs([`${ts()} load_tensors: loading model tensors ... ${i * 20}%`]);
      await sleep(450);
    }
    setStatus({ state: "ready", loadProgress: null });
    pushLogs([`${ts()} main: server is listening on http://127.0.0.1:${settings.port} - starting the main loop`]);
  })();
  return status;
}

// ---------------- fit calc (rough, mock only) ----------------
const kvBytesPer = (t: string) => (t === "q8_0" ? 1.0625 : t === "q4_0" ? 0.5625 : 2);
function fit(input: FitInput): FitResult {
  const m = models.find((x) => x.primaryPath === input.modelPath);
  if (!m) throw `No model at ${input.modelPath}. Pick a model on disk.`;
  const meta = m.meta!;
  const total = 24 * GiB;
  const vramTotal = input.vramTotalOverride ?? total;
  const size = m.sizeBytes;
  const blocks = meta.blockCount;
  let cpuW = 0;
  if (meta.isMoe && meta.expertTensorBytes) {
    const perLayer = meta.expertTensorBytes / blocks;
    const n = input.cpuMoeAll ? blocks : Math.min(blocks, input.nCpuMoe ?? 0);
    cpuW = perLayer * n;
  }
  const ngl = Math.min(input.ngl, blocks + 1);
  if (ngl <= blocks) cpuW += ((size - cpuW) * (blocks + 1 - ngl)) / (blocks + 1);
  const gpuW = size - cpuW;
  const kvLayers = meta.kvLayers ?? blocks;
  const hk = meta.headCountKv ?? 8;
  const kl = meta.keyLength ?? 128;
  const vl = meta.valueLength ?? 128;
  const kvb = kvLayers * input.ctxSize * hk * (kl * kvBytesPer(input.cacheTypeK) + vl * kvBytesPer(input.cacheTypeV));
  const rec = (meta.recurrentStateBytes ?? 0) * input.parallel;
  const over = 0.85 * GiB + input.ctxSize * 1024 * (input.flashAttn === false ? 3 : 1) + (input.ubatch ?? 512) * 0.4 * MiB;
  const reserve = 1.5 * GiB;
  const need = gpuW + kvb + rec + over + reserve;
  const verdict = need <= vramTotal * 0.94 ? "fits" : need <= vramTotal ? "tight" : "wont_fit";
  const sugg: Suggestion[] = [];
  if (verdict !== "fits") {
    const room = vramTotal * 0.94 - (gpuW + rec + over + reserve);
    const perTok = kvb / input.ctxSize;
    let c = Math.floor(room / perTok / 4096) * 4096;
    if (c >= 4096 && c < input.ctxSize) sugg.push({ kind: "lower_ctx", label: `Lower context to ${Math.round(c / 1024)}K`, patch: { ctxSize: c } });
    if (input.cacheTypeK === "f16" || input.cacheTypeV === "f16")
      sugg.push({ kind: "kv_q8", label: "Quantize KV cache to q8_0", patch: { cacheTypeK: "q8_0", cacheTypeV: "q8_0" } });
    else if (input.cacheTypeK !== "q4_0")
      sugg.push({ kind: "kv_q4", label: "Quantize KV cache to q4_0", patch: { cacheTypeK: "q4_0", cacheTypeV: "q4_0" } });
    if (meta.isMoe && !input.cpuMoeAll) {
      const n = (input.nCpuMoe ?? 0) + 4;
      sugg.push({ kind: "more_cpu_moe", label: `Raise --n-cpu-moe to ${n}`, patch: { nCpuMoe: n } });
    }
    sugg.push({ kind: "smaller_quant", label: "Pick a smaller quant (e.g. UD-Q4_K_XL)", patch: {} });
    sugg.push({ kind: "close_apps", label: "Close other GPU apps (frees ~1.4 GiB)", patch: {} });
  }
  const partialDense = !meta.isMoe && ngl <= blocks;
  const speedHint = meta.isMoe && cpuW > 0
    ? `${input.cpuMoeAll ? "All" : input.nCpuMoe} expert layers in RAM: expect a modest slowdown (~10–25%).`
    : partialDense
      ? "Dense model partly in RAM: generation will be several times slower. Lower context or pick a smaller quant instead."
      : "Everything on GPU: full speed.";
  const isActive = status.state === "ready" && status.modelPath === input.modelPath;
  return {
    weightsGpu: Math.round(gpuW),
    weightsCpu: Math.round(cpuW),
    kvBytes: Math.round(kvb),
    kvAccuracy: meta.kvLayersSource === "exact" ? "estimate" : "upper_bound",
    recurrentBytes: Math.round(rec),
    computeOverhead: Math.round(over),
    overheadAccuracy: isActive ? "measured" : "estimate",
    displayReserve: Math.round(reserve),
    vramNeeded: Math.round(need),
    vramTotal,
    vramFreeNow: Math.round(vramTotal - hist.vramUsed[hist.vramUsed.length - 1]),
    ramNeeded: Math.round(cpuW + 2.1 * GiB),
    ramTotal: 64 * GiB,
    verdict,
    suggestions: sugg,
    speedHint,
    measuredVram: isActive ? Math.round(21.45 * GiB) : null,
    errorPct: isActive ? -3.1 : null,
    notes: [
      meta.kvLayersSource === "exact"
        ? `Hybrid attention: KV cache on ${kvLayers} of ${blocks} layers (read from metadata).`
        : `Layer pattern not in metadata: assumed KV on all ${blocks} layers (upper bound).`,
      ...(input.parallel > 1 ? [`${input.parallel} slots share ${Math.round(input.ctxSize / 1024)}K context (${Math.round(input.ctxSize / input.parallel / 1024)}K each).`] : []),
    ],
  };
}

// ---------------- HF ----------------
function hfFiles(repo: string): HfFile[] {
  const base = repo.split("/")[1]?.replace(/-GGUF$/i, "") ?? "model";
  const big = /35B|27B|32B|70B/i.test(repo);
  const scale = big ? 1 : 0.15;
  const one = (q: string, gib: number): HfFile => ({ path: `${base}-${q}.gguf`, size: Math.round(gib * scale * GiB), quant: q, isMmproj: false, shardGroup: null });
  const split = (q: string, gib: number, n: number): HfFile[] =>
    Array.from({ length: n }, (_, i) => ({
      path: `${q}/${base}-${q}-0000${i + 1}-of-0000${n}.gguf`,
      size: Math.round((gib * scale * GiB) / n),
      quant: q,
      isMmproj: false,
      shardGroup: `${q}/${base}-${q}`,
    }));
  return [
    one("UD-Q2_K_XL", 12.9),
    one("UD-Q3_K_XL", 15.4),
    one("UD-Q4_K_XL", 20.1),
    one("Q4_K_M", 19.9),
    one("UD-Q5_K_M", 24.7),
    one("Q6_K", 27.6),
    ...split("Q8_0", 35.7, 2),
    ...split("BF16", 64.6, 3),
    { path: "mmproj-F16.gguf", size: Math.round(0.86 * GiB), quant: "F16", isMmproj: true, shardGroup: null },
    { path: "mmproj-BF16.gguf", size: Math.round(0.86 * GiB), quant: "BF16", isMmproj: true, shardGroup: null },
  ];
}

// ---------------- dispatcher ----------------
type Handlers = { [K in CommandName]: (args: Args<K>) => Ret<K> | Promise<Ret<K>> };

const h: Handlers = {
  settings_get: () => settings,
  settings_save: ({ settings: s }) => {
    if (s.lanMode && !s.apiKey.trim()) throw "LAN mode needs an API key. Generate one before binding to 0.0.0.0.";
    settings = clone(s);
    return settings;
  },
  paths_get: () => ({
    root: ROOT,
    models: `${ROOT}\\models`,
    engines: `${ROOT}\\llama.cpp`,
    logs: `${ROOT}\\logs`,
    data: `${ROOT}\\neurallama`,
  }),
  profile_upsert: ({ profile }) => {
    const i = settings.profiles.findIndex((p) => p.id === profile.id);
    if (i >= 0) settings.profiles[i] = clone(profile);
    else settings.profiles.push(clone(profile));
    return settings;
  },
  profile_delete: ({ id }) => {
    settings.profiles = settings.profiles.filter((p) => p.id !== id);
    if (settings.activeProfile === id) settings.activeProfile = null;
    return settings;
  },
  profile_set_active: ({ id }) => {
    if (!settings.profiles.some((p) => p.id === id)) throw `No profile with id ${id}`;
    settings.activeProfile = id;
    return settings;
  },
  api_key_regenerate: () => {
    settings.apiKey = "nl-" + rid() + rid() + rid().slice(0, 8);
    return settings;
  },
  app_info: () => ({ version: "0.1.0", os: "windows (mock)" }),
  tip_dismiss: ({ id }) => {
    if (!settings.dismissedTips.includes(id)) settings.dismissedTips.push(id);
    return settings;
  },
  open_path: () => null,

  server_status: () => status,
  server_start: async ({ profileId }) => {
    await sleep(300);
    if (status.state === "ready" || status.state === "loading") throw "The server is already running.";
    return bootServer(profileId);
  },
  server_stop: async () => {
    await sleep(400);
    setStatus({ state: "stopped", pid: null, startedAt: null, loadProgress: null });
    pushLogs([`${ts()} srv    operator(): operator(): cleaning up before exit...`]);
    return status;
  },
  server_restart: async () => {
    await sleep(400);
    setStatus({ state: "stopped", pid: null });
    return bootServer(status.profileId);
  },
  server_log_tail: ({ lines }) => {
    start();
    return logs.slice(-lines);
  },
  server_command_preview: ({ profile: p }) => {
    const exe = `${ROOT}\\llama.cpp\\${settings.activeEngine ?? "b7012-cuda-13.1"}\\llama-server.exe`;
    const a: string[] = ["-m", p.modelPath];
    if (p.mmprojPath) a.push("--mmproj", p.mmprojPath);
    a.push("-ngl", String(p.ngl));
    if (p.cpuMoeAll) a.push("--cpu-moe");
    else if (p.nCpuMoe != null) a.push("--n-cpu-moe", String(p.nCpuMoe));
    a.push("--ctx-size", String(p.ctxSize), "-ctk", p.cacheTypeK, "-ctv", p.cacheTypeV);
    if (p.flashAttn) a.push("-fa", "on");
    if (p.threads != null) a.push("-t", String(p.threads));
    if (p.threadsBatch != null) a.push("-tb", String(p.threadsBatch));
    if (p.batch != null) a.push("-b", String(p.batch));
    if (p.ubatch != null) a.push("-ub", String(p.ubatch));
    a.push("--parallel", String(p.parallel));
    if (p.jinja) a.push("--jinja");
    if (p.temp != null) a.push("--temp", String(p.temp));
    if (p.topP != null) a.push("--top-p", String(p.topP));
    if (p.topK != null) a.push("--top-k", String(p.topK));
    if (p.minP != null) a.push("--min-p", String(p.minP));
    a.push("--alias", settings.alias, "--host", settings.lanMode ? "0.0.0.0" : settings.host, "--port", String(settings.port), "--api-key", "••••••", "--metrics");
    if (p.extraArgs.trim()) a.push(...p.extraArgs.trim().split(/\s+/));
    const q = (s: string) => (/[\s"]/.test(s) ? `"${s.replace(/"/g, '\\"')}"` : s);
    return { program: exe, args: a, display: [exe, ...a].map(q).join(" ") };
  },
  server_next_free_port: () => (settings.port === 8080 ? 8081 : settings.port + 1),

  engine_installed: () => engines,
  engine_releases: async () => {
    await sleep(350);
    return releases();
  },
  engine_install: ({ tag, assetName }) => {
    const a = assetsFor(tag).find((x) => x.name === assetName);
    if (!a) throw `Asset ${assetName} not found in ${tag}.`;
    const folder = a.flavor === "cuda" ? `${tag}-cuda-${a.cudaVersion}` : `${tag}-${a.flavor}`;
    return runStepJob(
      "engine",
      `llama.cpp ${tag} (${a.flavor}${a.cudaVersion ? " " + a.cudaVersion : ""})`,
      ["Downloading…", "Downloading cudart…", "Verifying size…", "Extracting…", "Installed"],
      700,
      () => {
        if (!engines.some((e) => e.tag === folder))
          engines.unshift({ tag: folder, flavor: a.flavor, dir: `${ROOT}\\llama.cpp\\${folder}`, serverExe: `${ROOT}\\llama.cpp\\${folder}\\llama-server.exe`, active: false });
      },
    );
  },
  engine_set_active: ({ tag }) => {
    if (!engines.some((e) => e.tag === tag)) throw `No installed build ${tag}.`;
    engines = engines.map((e) => ({ ...e, active: e.tag === tag }));
    settings.activeEngine = tag;
    return settings;
  },
  engine_remove: ({ tag }) => {
    const e = engines.find((x) => x.tag === tag);
    if (e?.active && status.state !== "stopped") throw "That build is active and the server is running. Stop the server or switch builds first.";
    engines = engines.filter((x) => x.tag !== tag);
    return engines;
  },
  engine_driver_info: (a) => {
    const cv = a?.cudaVersion ?? null;
    const warnings: string[] = [];
    if (cv && Number(cv) < 12.8) warnings.push(`CUDA ${cv} builds have no Blackwell (sm_120) kernels. RTX 50-series needs a CUDA 13.x build.`);
    if (cv && Number(cv) > 13.1) warnings.push(`CUDA ${cv} needs a newer driver than 581.57.`);
    return { gpuName: "NVIDIA GeForce RTX 5090 Laptop GPU", driverVersion: "581.57", cudaDriverVersion: "13.1", computeCap: "12.0", warnings };
  },

  models_scan: async () => {
    await sleep(250);
    return models;
  },
  model_meta: ({ path }) => {
    const m = models.find((x) => x.primaryPath === path);
    if (!m?.meta) throw `Could not read GGUF header: ${path}`;
    return m.meta;
  },
  model_delete: ({ id }) => {
    if (models.find((m) => m.id === id)?.primaryPath === status.modelPath && status.state === "ready")
      throw "This model is being served. Stop the server first.";
    models = models.filter((m) => m.id !== id);
    return models;
  },
  model_serve: async ({ id }) => {
    const m = models.find((x) => x.id === id);
    if (!m) throw "Model not found.";
    let p = settings.profiles.find((x) => x.modelPath === m.primaryPath);
    if (!p) {
      p = { ...baseProfile(), name: m.name, modelPath: m.primaryPath, mmprojPath: m.mmprojPath };
      settings.profiles.push(p);
    }
    settings.activeProfile = p.id;
    setStatus({ state: "stopped", pid: null });
    await sleep(200);
    return bootServer(p.id);
  },
  hf_search: async ({ query }) => {
    await sleep(500);
    const q = query.toLowerCase();
    const all = [
      { id: "unsloth/Qwen3.6-35B-A3B-GGUF", downloads: 812_403, likes: 1_204, lastModified: "2026-10-30T11:00:00Z", gated: false },
      { id: "unsloth/Qwen3.8-27B-GGUF", downloads: 455_120, likes: 760, lastModified: "2026-11-04T08:10:00Z", gated: false },
      { id: "bartowski/Qwen_Qwen3.8-27B-GGUF", downloads: 120_884, likes: 211, lastModified: "2026-11-05T19:22:00Z", gated: false },
      { id: "unsloth/Qwen3.6-4B-GGUF", downloads: 302_002, likes: 388, lastModified: "2026-10-22T07:45:00Z", gated: false },
      { id: "google/gemma-4-12b-it-GGUF", downloads: 98_220, likes: 540, lastModified: "2026-09-12T12:00:00Z", gated: true },
    ];
    return all.filter((m) => q.split(/\s+/).every((w) => m.id.toLowerCase().includes(w)));
  },
  hf_files: async ({ repoId }) => {
    await sleep(400);
    if (!/^[\w.-]+\/[\w.-]+$/.test(repoId)) throw `"${repoId}" is not a repo id (expected org/name).`;
    if (/gemma-4/i.test(repoId) && !hfTokenStored) throw "This repo is gated. Accept its terms on huggingface.co and set an HF token in Settings.";
    return hfFiles(repoId);
  },
  download_start: ({ repoId, files }) => {
    const all = hfFiles(repoId);
    const total = files.reduce((s, f) => s + (all.find((x) => x.path === f)?.size ?? 0), 0);
    const id = rid();
    const q = all.find((x) => x.path === files[0])?.quant ?? files[0];
    pushJob({ jobId: id, kind: "download", label: `${repoId} · ${q}`, done: 0, total, state: "running", message: "Starting…" });
    return id;
  },
  download_pause: ({ jobId }) => {
    const j = jobs.get(jobId);
    if (j) {
      j.state = "paused";
      j.message = null;
      emit("job-progress", j);
    }
    return null;
  },
  download_resume: ({ jobId }) => {
    const j = jobs.get(jobId);
    if (j) {
      j.state = "running";
      emit("job-progress", j);
    }
    return null;
  },
  download_cancel: ({ jobId }) => {
    const j = jobs.get(jobId);
    if (j && j.kind !== "download") {
      j._cancel = true;
      return null;
    }
    if (j) {
      j.state = "cancelled";
      j.message = "Cancelled; partial file removed";
      emit("job-progress", j);
    }
    return null;
  },
  downloads_list: () => {
    start();
    return [...jobs.values()].filter((j) => j.kind === "download");
  },
  hf_token_set: ({ token }) => {
    hfTokenStored = !!token;
    return null;
  },
  hf_token_status: () => hfTokenStored,
  disk_free: () => Math.round(412.6 * GiB),

  telemetry_snapshot: () => {
    start();
    return snapshot();
  },
  stats_list: () => stats,
  stats_reset: ({ model }) => {
    stats = model ? stats.filter((s) => s.model !== model) : [];
    return stats;
  },
  gpu_processes: () => [
    { pid: status.pid ?? 0, name: "llama-server.exe", vramBytes: Math.round(20.1 * GiB) },
    { pid: 9120, name: "chrome.exe", vramBytes: Math.round(0.82 * GiB) },
    { pid: 1408, name: "dwm.exe", vramBytes: Math.round(0.61 * GiB) },
    { pid: 13300, name: "Code.exe", vramBytes: Math.round(0.33 * GiB) },
  ],
  fit_estimate: async ({ input }) => {
    await sleep(60);
    return fit(input);
  },
  autotune_start: ({ profileId }) => {
    const p = settings.profiles.find((x) => x.id === profileId);
    if (!p) throw "Save the profile before auto-tuning.";
    return runStepJob(
      "autotune",
      `Auto-tune ${p.name}`,
      [
        "Launching with --n-cpu-moe 8…",
        "--n-cpu-moe 8: VRAM 21.5 GiB (90%), trying 6",
        "--n-cpu-moe 6: VRAM 23.4 GiB (98%), too high, trying 7",
        "--n-cpu-moe 7: VRAM 22.1 GiB (92%), converged",
        "Saved --n-cpu-moe 7 to profile",
      ],
      1200,
      () => {
        p.nCpuMoe = 7;
      },
    );
  },
  autotune_cancel: ({ jobId }) => {
    const j = jobs.get(jobId);
    if (j) j._cancel = true;
    return null;
  },
  benchmark_run: async ({ profileId }) => {
    const jobId = rid();
    const progress: JobProgress = { jobId, kind: "benchmark", label: "Benchmark", done: 0, total: null, state: "running", message: null };
    jobs.set(jobId, progress);
    emit("job-progress", progress);
    if (status.state !== "ready") throw "Start the server first; the benchmark runs through it.";
    for (let elapsed = 0; elapsed < 2500; elapsed += 100) {
      await sleep(100);
      if (jobs.get(jobId)?._cancel) {
        jobs.set(jobId, { ...progress, state: "cancelled", message: "Benchmark cancelled." });
        emit("job-progress", jobs.get(jobId)!);
        throw "Benchmark cancelled.";
      }
    }
    const r = { promptTokens: 2048, genTokens: 256, promptTps: 2280 + Math.random() * 120, genTps: 90 + Math.random() * 8, totalMs: 3540 };
    const p = settings.profiles.find((x) => x.id === (profileId ?? settings.activeProfile));
    if (p) p.bench = { at: new Date().toISOString(), promptTps: r.promptTps, genTps: r.genTps };
    jobs.set(jobId, { ...progress, state: "done" });
    emit("job-progress", jobs.get(jobId)!);
    return r;
  },

  connect_info: () => {
    const base = `http://${settings.lanMode || settings.host === "0.0.0.0" ? "127.0.0.1" : settings.host}:${settings.port}/v1`;
    const k = settings.apiKey;
    const m = settings.alias;
    return {
      baseUrl: base,
      apiKey: k,
      modelId: m,
      lanUrls: settings.lanMode ? [`http://192.168.1.42:${settings.port}/v1`, `http://172.29.80.1:${settings.port}/v1`] : [],
      snippets: {
        curl: `curl ${base}/chat/completions \\\n  -H "Authorization: Bearer ${k}" \\\n  -H "Content-Type: application/json" \\\n  -d '{"model":"${m}","messages":[{"role":"user","content":"Hello"}]}'`,
        python: `from openai import OpenAI\n\nclient = OpenAI(base_url="${base}", api_key="${k}")\nr = client.chat.completions.create(\n    model="${m}",\n    messages=[{"role": "user", "content": "Hello"}],\n)\nprint(r.choices[0].message.content)`,
        node: `import OpenAI from "openai";\n\nconst client = new OpenAI({ baseURL: "${base}", apiKey: "${k}" });\nconst r = await client.chat.completions.create({\n  model: "${m}",\n  messages: [{ role: "user", content: "Hello" }],\n});\nconsole.log(r.choices[0].message.content);`,
      },
    };
  },
  connect_test: async () => {
    await sleep(700);
    if (status.state !== "ready")
      return { ok: false, status: null, message: `Connection refused: nothing is listening on 127.0.0.1:${settings.port}.`, latencyMs: 3 };
    return { ok: true, status: 200, message: "Hello! How can I help you today?", latencyMs: 412 };
  },
  hermes_targets: async () => {
    await sleep(200);
    return hermes;
  },
  hermes_push: async ({ targetId, contextLength }) => {
    await sleep(500);
    const t = hermes.find((x) => x.id === targetId);
    if (!t) throw "Unknown Hermes target.";
    t.pointsHere = true;
    t.currentProvider = "neurallama";
    t.currentModel = settings.alias;
    return {
      targetId,
      ok: true,
      backupPath: `${t.configPath}.20261111-163012.bak`,
      message: `Wrote provider "neurallama" (model ${settings.alias}${contextLength ? `, context_length ${contextLength}` : ""}) to ${t.configPath}.`,
    };
  },
  continue_push: async () => {
    await sleep(400);
    return {
      ok: true,
      path: "C:\\Users\\you\\.continue\\config.yaml",
      backupPath: "C:\\Users\\you\\.continue\\config.yaml.20261111-163044.bak",
      message: `Added model "Neurallama (${settings.alias})" to Continue.`,
    };
  },
};

export async function invoke(cmd: CommandName, args: unknown): Promise<unknown> {
  await sleep(40);
  const fn = h[cmd] as (a: unknown) => unknown;
  if (!fn) throw `mock: unknown command ${cmd}`;
  return clone(await fn(args ?? {}));
}
