# Backend ↔ UI contract

The single source of truth for the Tauri commands and events. Payload types are the Rust
structs in `src-tauri/src/*/mod.rs` (serde `camelCase`; enums serialize as lowercase or
snake_case strings, as annotated). The UI mirrors them in `src/lib/types.ts` and calls them
through `src/lib/api.ts`.

Tauri converts command argument names: Rust `profile_id` ⇄ JS `{ profileId }`.
Every command returns `Result<T, String>`; the error string is plain English for the UI.

## App / settings (`app_commands.rs`)

| Command | Args | Returns |
|---|---|---|
| `settings_get` | — | `Settings` |
| `settings_save` | `settings: Settings` | `Settings` |
| `paths_get` | — | `Paths` |
| `profile_upsert` | `profile: Profile` | `Settings` |
| `profile_delete` | `id` | `Settings` |
| `profile_set_active` | `id` | `Settings` |
| `api_key_regenerate` | — | `Settings` |
| `app_info` | — | `{ version, os }` |
| `tip_dismiss` | `id` | `Settings` |
| `open_path` | `path` | `()` — opens Explorer |

## Server (`server/commands.rs`)

| Command | Args | Returns |
|---|---|---|
| `server_status` | — | `ServerStatus` |
| `server_start` | `profile_id: Option<String>` (None = active profile) | `ServerStatus` — returns once spawned; readiness arrives via events |
| `server_stop` | — | `ServerStatus` |
| `server_restart` | — | `ServerStatus` |
| `server_log_tail` | `lines: usize` | `Vec<String>` |
| `server_command_preview` | `profile: Profile` | `CommandPreview` |
| `server_next_free_port` | — | `u16` |

Events: `server-status` → `ServerStatus` (on every state change), `server-log` → `Vec<String>` (new lines, batched ≤ 4 Hz).

## Engine (`engine/commands.rs`)

| Command | Args | Returns |
|---|---|---|
| `engine_installed` | — | `Vec<InstalledEngine>` |
| `engine_releases` | — | `Vec<Release>` (respects `settings.engine_channel`) |
| `engine_install` | `tag, asset_name` | job id `String` (progress via `job-progress`, kind `engine`) |
| `engine_set_active` | `tag` | `Settings` |
| `engine_remove` | `tag` | `Vec<InstalledEngine>` (refuses the active one while the server runs) |
| `engine_driver_info` | `cuda_version: Option<String>` (selected asset) | `DriverInfo` |

## Models (`models/commands.rs`)

| Command | Args | Returns |
|---|---|---|
| `models_scan` | `with_meta: bool` | `Vec<LocalModel>` |
| `model_meta` | `path` | `GgufSummary` |
| `model_delete` | `id` | `Vec<LocalModel>` |
| `model_serve` | `id` | `ServerStatus` — finds/creates a profile for the model, makes it active, (re)starts |
| `hf_search` | `query` | `Vec<HfModel>` (GGUF repos only) |
| `hf_files` | `repo_id` | `Vec<HfFile>` (.gguf files only) |
| `download_start` | `repo_id, files: Vec<String>` | job id (kind `download`) |
| `download_pause` / `download_resume` / `download_cancel` | `job_id` | `()` |
| `downloads_list` | — | `Vec<JobProgress>` |
| `hf_token_set` | `token: Option<String>` (None = delete) | `()` — Windows Credential Manager |
| `hf_token_status` | — | `bool` |
| `disk_free` | — | `u64` bytes free on the models drive |

## Telemetry + fit (`telemetry/commands.rs`, `fit/commands.rs`)

| Command | Args | Returns |
|---|---|---|
| `telemetry_snapshot` | — | `Snapshot` |
| `stats_list` | — | `Vec<ModelStats>` |
| `stats_reset` | `model: Option<String>` | `Vec<ModelStats>` |
| `gpu_processes` | — | `Vec<GpuProcess>` (top VRAM consumers) |
| `fit_estimate` | `input: FitInput` | `FitResult` |
| `autotune_start` | `profile_id` | job id (kind `autotune`) |
| `autotune_cancel` | `job_id` | `()` |
| `benchmark_run` | `profile_id: Option<String>` | `BenchResult` (also saved to the profile); emits a `benchmark` job, cancellable with `download_cancel` |

Events: `telemetry` → `Snapshot` (1 Hz, only while the window is visible is fine but not required).

## Clients (`clients/commands.rs`)

| Command | Args | Returns |
|---|---|---|
| `connect_info` | — | `ConnectInfo` |
| `connect_test` | — | `TestResult` |
| `hermes_targets` | — | `Vec<HermesTarget>` |
| `hermes_push` | `target_id, context_length: Option<u32>` | `HermesPushResult` |
| `continue_push` | — | `ClientWriteResult` |

## Jobs (all kinds)

Event `job-progress` → `JobProgress { jobId, kind, label, done, total, state, message }`,
`state` ∈ `queued | running | paused | done | error | cancelled`.
