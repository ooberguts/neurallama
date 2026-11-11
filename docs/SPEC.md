# Build Brief: "Neurallama" — a Windows control panel for llama-server (Tauri)

App name: **Neurallama** (styled NeuraLlama in the logo; repo `neurallama`).

Hand this whole document to a fresh Codex/agent session. It is self-contained.

## 1. What we're building

A small Windows desktop app (Tauri 2 + a web UI) that starts, stops, monitors and configures **llama.cpp's `llama-server`**, manages local GGUF models, and wires up clients. It is the Windows/llama.cpp equivalent of the macOS app https://github.com/ooberguts/oMLX-widget (an AppKit + WKWebView panel for the oMLX inference server). Read that repo's README first and mirror its feature set and feel: a compact window that fits in a screen corner, tabs for Monitor / Models / Limits / Connect, no bloat.

Differences from the reference:
- Engine is `llama-server` (llama.cpp), not oMLX. NVIDIA GPU on Windows, not Apple Silicon.
- Memory model is VRAM + system RAM (with MoE expert offload), not unified memory.
- Tauri (Rust backend + WebView2 frontend) instead of Swift/AppKit.

## 2. Target machine (design for this, support others)

- Windows 11 x64, Intel Core Ultra 9 275HX, 64 GB RAM, NVIDIA RTX 5090 Laptop GPU with 24 GB VRAM (Blackwell, sm_120).
- Typical models: Qwen3.6-35B-A3B (MoE) and Qwen3.8-27B (dense) as GGUF, from Hugging Face (`unsloth/Qwen3.6-35B-A3B-GGUF`, `unsloth/Qwen3.8-27B-GGUF`).
- Must also work on smaller NVIDIA GPUs and, as a fallback, CPU-only or Vulkan builds of llama.cpp.

## 3. Stack and constraints

- **Tauri 2**, Rust backend, plain TypeScript + a light framework (Svelte or React; pick one and keep deps few). No Electron.
- Single-file-ish UI is fine; prioritize small install size and fast startup.
- Windows only for v1, but keep paths and process code behind small modules so macOS/Linux could follow.
- Installer: Tauri's NSIS or MSI bundle. Per-user install, no admin required.
- App data root default `%USERPROFILE%\AI` (override with `NEURALLAMA_HOME`):

```
AI\
  models\<org>\<name>\*.gguf     downloaded weights (HF layout, same idea as the reference)
  llama.cpp\                     downloaded llama-server build(s), one folder per release tag
  logs\                          server logs
  neurallama\settings.json       app settings and per-model launch profiles
  neurallama\stats.json          lifetime tokens/sec stats
```

## 4. Core backend (Rust)

### 4.1 Process control
- Start `llama-server.exe` as a child process with `CREATE_NO_WINDOW`; put it in a Windows **Job Object** with kill-on-close so it never outlives the app.
- Stop = graceful (terminate, short wait, then kill). Restart. Status (running / loading / ready / crashed).
- Capture stdout/stderr to a rolling log file and stream the tail to the UI. Parse the log for: model load progress, "server is listening", CUDA/VRAM allocation lines, and errors (e.g. out-of-memory) and surface a plain-English message.
- Optional "start with Windows" (per-user Run key or Task Scheduler) and "start server when app opens". Optional system tray icon with Start/Stop/Open.
- Detect an already-running llama-server on the configured port and attach instead of spawning a duplicate.

### 4.2 Launch profiles
A profile is per-model saved settings that become CLI args. Build the arg list in one place and show the final command line in the UI (copyable).

| Setting | Flag |
|---|---|
| Model file | `-m <path>` (plus `--mmproj <path>` if a vision projector exists) |
| GPU layers | `-ngl 99` (default all) |
| MoE experts on CPU | `--n-cpu-moe N` (or `--cpu-moe` for all) |
| Context size | `--ctx-size N` |
| KV cache types | `-ctk` / `-ctv` (`f16`, `q8_0`, `q4_0`) |
| Flash attention | `-fa on` |
| Threads | `-t`, `-tb` |
| Batch sizes | `-b`, `-ub` |
| Parallel slots | `--parallel N` (default 1) |
| Jinja chat template | `--jinja` (default on) |
| Sampling defaults | `--temp`, `--top-p`, `--top-k`, `--min-p` |
| Alias | `--alias local` so clients use a stable model id |
| Host/port/API key | `--host 127.0.0.1 --port 8080 --api-key ...` |
| Metrics | `--metrics` (always on; needed for monitoring) |
| Extra args | free-text field, appended verbatim |

Ship sensible presets for the target machine: "Qwen3.8-27B Q4_K_M, 64K" and "Qwen3.6-35B-A3B Q5_K_M, 128K, `--n-cpu-moe 8`".

### 4.3 llama.cpp binary manager
- Query GitHub Releases for `ggml-org/llama.cpp`, pick the Windows CUDA asset (`llama-*-bin-win-cuda-*-x64.zip` plus the matching `cudart-*` zip if separate), and also offer Vulkan and CPU builds.
- Download, verify size, extract to `AI\llama.cpp\<tag>\`, keep several versions, let the user pick the active one, and roll back. Show current version and "newer release available" like the reference's Server row. Updating asks twice because a restart drops in-flight requests.
- Detect the installed NVIDIA driver / CUDA compatibility and warn if the chosen build needs a newer driver. RTX 50-series needs a CUDA 12.8+ build.
- Never run unsigned downloads silently: show the source URL and size before downloading.

### 4.4 Model manager
- Scan `AI\models` recursively for `.gguf`; group split files (`-00001-of-0000N`) as one model; detect `mmproj*.gguf` siblings; show size and org (from folder layout; flat folders are marked "source unknown").
- Download from Hugging Face by exact repo id + file/quant pick, or by search. Use the HF HTTP API (`/api/models?search=`, `/api/models/{id}/tree/main`) and resumable downloads (`Range` headers) with progress, pause/cancel, and disk-space check. Accept an optional HF token for gated repos, stored in Windows Credential Manager, never in plain settings.
- Two-click delete. "Copy ids" / "Copy links" for everything on disk, and a multi-id "Get" box, same as the reference.
- Read GGUF metadata (header only) for architecture, layer count, expert count, native context length, embedding/head dims, and quant type. Use it for the fit calculator.
- "Serve" = make this model active (restart server with its profile). Because llama-server serves one model per process by default, either restart on switch, or, if the installed build supports router mode / multiple models, use that. Keep this behind one interface so it can change. Optionally integrate `llama-swap` as an alternative backend later.

### 4.5 Telemetry
- **llama-server:** poll `/health`, `/props` (model, ctx, build info), `/slots` (per-slot state, tokens), and `/metrics` (Prometheus: prompt tokens, generated tokens, tokens/sec, requests processing/deferred, KV usage). Check actual endpoint names and fields against the installed build's README before relying on them; they change between releases. Degrade gracefully when one is missing.
- **GPU:** NVML (via the `nvml-wrapper` crate) for utilization %, VRAM used/total, temperature, power, clocks. Fall back to parsing `nvidia-smi --query-gpu=... --format=csv` if NVML fails. Also show system RAM used/free (via `sysinfo`) and the llama-server process's working set.
- Keep a 90-second ring buffer at ~1 Hz for GPU %, VRAM, and tokens/sec; persist per-model lifetime stats (tokens, requests, avg/peak/low tok/s) to `stats.json`.
- Tokens/sec: derive from `/metrics` counters and slot state while a request is in flight (counters may only move on completion in some builds, so prefer live slot data if available).

## 5. UI (tabs)

Compact window (~480x640 default, resizable, remember position, optional always-on-top and tray). Dark/light follows Windows. Fast, no layout jumps.

### Monitor
- Server status pill with Start / Stop / Restart.
- VRAM bar (used / total) and system RAM bar, with warning color near the ceiling.
- Rolling 90 s graphs: GPU utilization, tokens/sec.
- Request counts, active slots, KV cache fill.
- Per-model lifetime stats table.
- Server row: llama.cpp version, newer-release check, update (confirm twice), roll back.
- Live log tail with filter and "copy last 200 lines".

### Models
- List on disk with size, org, quant, arch, mmproj indicator, and a **"fits?"** badge (green / tight / won't fit) from the fit calculator.
- Get by repo id or HF search; download queue with progress.
- Serve, delete, copy ids/links.

### Limits (the fit calculator — the app's most valuable feature)
Forecast memory for a model + context length + KV precision, shown against VRAM and RAM:
- Weights: GGUF file size, split into GPU-resident vs. CPU-resident given `-ngl` and `--n-cpu-moe`.
- KV cache: computed from GGUF metadata (layers, KV heads, head dim, context, KV type). Hybrid/linear-attention architectures (Qwen3.5/3.6/3.8 style Gated DeltaNet) only have full-attention KV on some layers plus a fixed recurrent state, so read the layer pattern from metadata rather than assuming every layer; label accuracy as **measured**, **estimate** or **upper bound**, like the reference. Validate against the real VRAM reported after load and show the error.
- Compute buffers / CUDA context overhead (measure from the running server's logged allocations and `nvidia-smi`), plus a Windows display reserve (~1.5–2 GB).
- Suggestions that **stage** (not apply) changes into the controls: lower context to the largest that fits, quantize KV to q8_0 or q4_0, increase `--n-cpu-moe` by N, pick a smaller quant, close other GPU apps (list top VRAM consumers via NVML).
- Speed hint: MoE with experts in RAM = modest slowdown; dense with layers in RAM = steep slowdown. Warn when a dense model is partially offloaded.
- One-click "auto-tune `--n-cpu-moe`": start at a guess, relaunch, read VRAM, adjust until ~92% of VRAM, then save to the profile. Run in the background with clear progress and a cancel button.
- Show a one-time tip: NVIDIA Control Panel → "CUDA - Sysmem Fallback Policy" → "Prefer No Sysmem Fallback" so VRAM overflow errors out instead of silently crawling. Never change system settings automatically; show instructions only.
- A **Benchmark** button that runs a short fixed prompt through the server and reports prompt-processing and generation tok/s, saved per profile for comparison. (Optionally shell out to `llama-bench.exe`.)

### Connect
- Base URL (`http://127.0.0.1:8080/v1`), API key, model id (the alias), each with a copy button; ready-to-paste snippets for curl, Python `openai`, and Node.
- Client wiring panels (toggle-hidden) that write config with a timestamped backup first: Claude Code-compatible/OpenAI-compatible tools, Continue, Open WebUI, Cline, and Hermes Agent (see the reference README's Hermes panel for the provider-block approach: write a named provider, clear stale keys from the previous provider, never preselect or silently overwrite `default`). **Test** button sends a real chat completion and shows the server's own error text.
- Option to bind to `0.0.0.0` for LAN use, with a clear warning and a mandatory API key.

### Settings
Install root, port, host, API key (generated by default), start-with-Windows, auto-start server, tray behavior, HF token (credential manager), llama.cpp channel (stable/prerelease), telemetry off by default (no network calls except GitHub releases, Hugging Face, and the local server).

## 6. Quality bar and edge cases

- Port already in use → say so and offer the next free port.
- Model path with spaces/Unicode → quote properly.
- Crash loop detection: stop auto-restarting after 3 crashes in 60 s and show the last error.
- Out-of-memory at load → parse it, show the fit calculator with the failing config prefilled, and suggest the staged fix.
- Download interrupted → resume. Disk full → clear error before starting.
- All long operations are cancellable and never block the UI thread.
- Never run anything with admin rights; commands needing elevation are shown, not run.
- Don't send telemetry or personal data anywhere. HF token and API key never logged.
- Antivirus/SmartScreen: unsigned builds will warn; document it and keep the installer reproducible.

## 7. Suggested project layout

```
neurallama/
  src-tauri/
    src/
      main.rs, commands.rs
      server/     process.rs, args.rs, logs.rs, job_object.rs
      engine/     releases.rs, install.rs
      models/     scan.rs, gguf.rs, hf.rs, download.rs
      telemetry/  nvml.rs, metrics.rs, stats.rs
      fit/        calc.rs, autotune.rs
      clients/    hermes.rs, openai_snippets.rs
      settings.rs
    tauri.conf.json
  src/            UI (tabs: Monitor, Models, Limits, Connect, Settings)
  README.md
```

## 8. Milestones (build in this order, each shippable)

1. **Skeleton:** Tauri app, settings, start/stop a user-chosen `llama-server.exe` + GGUF with a hand-edited profile, log tail, Job Object cleanup.
2. **Monitor:** NVML + `/metrics` + `/slots`, graphs, status, lifetime stats.
3. **Models tab:** scan, GGUF metadata, HF search/download with resume, delete, serve.
4. **Engine manager:** download/update/rollback llama.cpp CUDA builds.
5. **Limits tab:** fit calculator, staged suggestions, `--n-cpu-moe` auto-tune, benchmark.
6. **Connect tab:** snippets, client wiring, Hermes panel, Test button.
7. **Polish:** tray, start-with-Windows, installer, README with screenshots.

## 9. Acceptance tests (on the target machine)

- Fresh install → engine manager downloads a CUDA build → download `unsloth/Qwen3.8-27B-GGUF` `UD-Q4_K_M` → Serve → Connect test returns a completion. All without touching a terminal.
- Qwen3.6-35B-A3B `UD-Q5_K_M` at 128K context: fit calculator predicts fit with `--n-cpu-moe` around 8; real VRAM after load is within ~10% of the forecast; auto-tune converges.
- Killing the app kills `llama-server`. Crashing the server shows a readable error, not a silent stop.
- Idle app CPU under 1%, memory under ~150 MB.

## 10. Open questions for the owner (decide early)

- UI framework: Svelte vs. React.
- Single model per server restart vs. integrating router mode / `llama-swap` for hot-swapping.
- Whether to bundle a default llama.cpp build or always download on first run (recommend download).
- App icon. (Name decided: Neurallama.)

## 11. References

- Reference app and feature spec: https://github.com/ooberguts/oMLX-widget
- llama.cpp server docs (verify endpoints and flags against the installed version): https://github.com/ggml-org/llama.cpp/tree/master/tools/server
- Tauri 2: https://tauri.app
- Hugging Face Hub API: https://huggingface.co/docs/hub/api
- Model repos: https://huggingface.co/unsloth/Qwen3.6-35B-A3B-GGUF and https://huggingface.co/unsloth/Qwen3.8-27B-GGUF
