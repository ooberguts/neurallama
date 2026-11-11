// App-wide stores (Svelte 5 runes). `initStores()` subscribes to backend events once.
import { api, events } from "../api";
import type { JobProgress, Paths, ServerStatus, Settings, Snapshot } from "../types";
import { toasts } from "./toasts.svelte";

const LOG_CAP = 2000;

class SettingsStore {
  value = $state<Settings | null>(null);
  paths = $state<Paths | null>(null);

  async load() {
    const [s, p] = await Promise.all([api.settingsGet(), api.pathsGet()]);
    this.value = s;
    this.paths = p;
  }
  /** Replace with a Settings value a command returned. */
  set(s: Settings) {
    this.value = s;
  }
  async save(next: Settings) {
    this.value = await api.settingsSave(next);
    return this.value;
  }
  /** Save a patch on top of the current settings. */
  async patch(p: Partial<Settings>) {
    if (!this.value) return;
    return this.save({ ...$state.snapshot(this.value), ...p } as Settings);
  }
  profile(id: string | null | undefined) {
    return this.value?.profiles.find((p) => p.id === id) ?? null;
  }
  get active() {
    return this.profile(this.value?.activeProfile);
  }
}

class ServerStore {
  status = $state<ServerStatus | null>(null);
  logs = $state<string[]>([]);

  get running() {
    const s = this.status?.state;
    return s === "ready" || s === "loading" || s === "starting" || s === "attached";
  }
  set(s: ServerStatus) {
    this.status = s;
  }
  addLogs(lines: string[]) {
    if (!lines.length) return;
    const next = this.logs.concat(lines);
    this.logs = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
  }
}

class TelemetryStore {
  snap = $state<Snapshot | null>(null);
}

class JobsStore {
  map = $state<Record<string, JobProgress>>({});

  set(j: JobProgress) {
    this.map[j.jobId] = j;
  }
  get(id: string | null | undefined) {
    return id ? (this.map[id] ?? null) : null;
  }
  ofKind(kind: string) {
    return Object.values(this.map).filter((j) => j.kind === kind);
  }
  /** Listeners fired once when a job reaches a terminal state. */
  #done = new Map<string, (j: JobProgress) => void>();
  onFinish(id: string, cb: (j: JobProgress) => void) {
    const j = this.map[id];
    if (j && isFinished(j)) cb(j);
    else this.#done.set(id, cb);
  }
  _emitFinish(j: JobProgress) {
    if (!isFinished(j)) return;
    const cb = this.#done.get(j.jobId);
    if (cb) {
      this.#done.delete(j.jobId);
      cb(j);
    }
  }
}

export function isFinished(j: JobProgress) {
  return j.state === "done" || j.state === "error" || j.state === "cancelled";
}

export const settings = new SettingsStore();
export const server = new ServerStore();
export const telemetry = new TelemetryStore();
export const jobs = new JobsStore();

let started = false;

export async function initStores() {
  if (started) return;
  started = true;

  await Promise.all([
    events.serverStatus((s) => server.set(s)),
    events.serverLog((lines) => server.addLogs(lines)),
    events.telemetry((s) => { telemetry.snap = s; }),
    events.jobProgress((j) => {
      jobs.set(j);
      jobs._emitFinish(j);
      if (j.state === "error" && j.message) toasts.push("error", `${j.label}: ${j.message}`);
    }),
  ]);

  const results = await Promise.allSettled([
    settings.load(),
    api.serverStatus().then((s) => server.set(s)),
    api.serverLogTail(500).then((l) => { server.logs = l; }),
    api.telemetrySnapshot().then((s) => { if (!telemetry.snap) telemetry.snap = s; }),
    api.downloadsList().then((list) => list.forEach((j) => jobs.set(j))),
  ]);
  for (const r of results) if (r.status === "rejected") toasts.error(r.reason);
}
