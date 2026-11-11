# Neurallama

A small Windows control panel for [llama.cpp](https://github.com/ggml-org/llama.cpp)'s
`llama-server`. It starts, stops and monitors the server, manages local GGUF models,
tells you whether a model will fit in your GPU before you load it, and wires up your
clients — including [Hermes Agent](https://github.com/NousResearch/hermes-agent) profiles.

It's the Windows / NVIDIA sibling of [oMLX-widget](https://github.com/ooberguts/oMLX-widget)
(macOS / Apple Silicon), built with [Tauri 2](https://tauri.app) — a Rust backend and a
WebView2 UI, so the installer is small and it starts fast.

> **Status: early (v0.1).** Built and unit-tested in CI on Windows, but not yet battle-tested
> on real NVIDIA hardware. Expect rough edges; issues welcome.

## Features

**Monitor** — server status with Start / Stop / Restart, VRAM and RAM bars, 90-second
graphs of GPU load and tokens/sec, request and slot counts, KV-cache fill, per-model
lifetime stats, a live log tail, and the llama.cpp version with update / roll back.

**Models** — every GGUF under `AI\models`, grouped by split files, with size, quant,
architecture, vision-projector badge and a **fits?** badge. Search Hugging Face or paste repo
ids, pick a quant, and download with resume, pause and cancel. Serve, delete, copy ids/links.

**Limits** — the fit calculator. Pick a model, context length, KV precision, GPU layers and
MoE experts on CPU, and see the VRAM forecast split into weights, KV cache, recurrent state,
compute overhead and a Windows display reserve — each labelled *measured*, *estimate* or
*upper bound*. Suggestions (lower context, quantize KV, move more experts to CPU) are staged
into the controls, never applied silently. One-click **auto-tune `--n-cpu-moe`** and a
**benchmark** button.

**Connect** — base URL, API key and model id with copy buttons; curl / Python / Node snippets;
a **Test** button that sends a real completion. Client wiring:
- **Hermes Agent** — finds every Hermes profile on the PC, native Windows
  (`%LOCALAPPDATA%\hermes`) *and* inside WSL distros (`~/.hermes`), and pushes this server into
  the one you choose as a named `neurallama` provider. Backs up `config.yaml` first, clears
  stale provider keys, never preselects or silently overwrites `default`. For WSL in NAT mode
  it uses the Windows host address and tells you what to open up.
- **Continue** — adds a "Neurallama (local)" model to `~/.continue/config.yaml` (with backup).
- **Open WebUI, Cline** — copyable settings.

**Engine manager** — downloads official llama.cpp Windows builds (CUDA, Vulkan or CPU) from
GitHub releases, keeps several versions side by side, switches and rolls back, and warns when
your NVIDIA driver is too old for the chosen CUDA build (RTX 50-series needs CUDA 12.8 or newer — today that means the CUDA 13 build).

Plus: system tray, start with Windows, LAN mode (with a mandatory API key), and the
server is placed in a Windows Job Object so it never outlives the app.

## Install

Download the installer from [Releases](https://github.com/ooberguts/neurallama/releases)
(or the latest `neurallama-windows-installer` artifact from
[Actions](https://github.com/ooberguts/neurallama/actions)). It installs per-user; no admin
needed.

The builds are unsigned, so Windows SmartScreen will warn on first run: choose
**More info → Run anyway**. You can always build it yourself (below).

First run: **Settings → Engine** → pick a CUDA build → Install. Then **Models → Get**, e.g.
`unsloth/Qwen3.8-27B-GGUF` → `UD-Q4_K_M` → Download → **Serve**. **Connect → Test** should
reply. No terminal needed.

## Where things live

Everything sits under `%USERPROFILE%\AI` (override with the `NEURALLAMA_HOME` environment
variable):

```
AI\models\<org>\<name>\*.gguf     downloaded weights
AI\llama.cpp\<build>\             llama-server builds, one folder each
AI\logs\                          server logs
AI\neurallama\settings.json       settings and per-model launch profiles
AI\neurallama\stats.json          lifetime tokens/sec stats
```

Your Hugging Face token (for gated repos) is kept in Windows Credential Manager, never in
`settings.json`.

## Privacy

No telemetry. The app only talks to GitHub releases (llama.cpp builds), Hugging Face (model
search and downloads), and your own llama-server.

## Tips

- In NVIDIA Control Panel → Manage 3D settings, set **CUDA – Sysmem Fallback Policy** to
  **Prefer No Sysmem Fallback**, so running out of VRAM fails loudly instead of silently
  crawling through system RAM. Neurallama shows this tip but never changes system settings.
- MoE models (e.g. Qwen3.6-35B-A3B) take expert offload to RAM gracefully with
  `--n-cpu-moe`; dense models slow down sharply when layers spill to RAM.

## Build from source

Requirements: Windows 10/11, [Rust](https://rustup.rs), Node.js 20+, and the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) (WebView2 and MSVC build tools).

```powershell
npm install
npx tauri dev      # run in development
npx tauri build    # installer in src-tauri\target\release\bundle\nsis
```

In a plain browser (`npm run dev`), the UI runs against a built-in mock backend, which is
handy for UI work on any OS.

Layout: `src-tauri/src/` holds the Rust backend (`server/`, `engine/`, `models/`,
`telemetry/`, `fit/`, `clients/`); `src/` holds the Svelte UI. The command and event contract
between them is in [docs/CONTRACT.md](docs/CONTRACT.md); the original product brief is
[docs/SPEC.md](docs/SPEC.md).

## License

MIT
