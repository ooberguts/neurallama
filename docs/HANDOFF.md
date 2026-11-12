# Neurallama — Windows installer handoff (2026-11-11)

The implementation is published at https://github.com/ooberguts/neurallama (public).
Windows CI passed for code commit `5cb03e5`:
https://github.com/ooberguts/neurallama/actions/runs/43141439957

The run passed **88 Windows tests** and built and uploaded the NSIS installer `Neurallama_0.1.0_x64-setup.exe` as
`neurallama-windows-installer`:
https://github.com/ooberguts/neurallama/actions/runs/43141439957/artifacts/13910196590

Installer SHA256: `100742782edf1a0ced7e9879d4f678f5f487cc97c405be4677067d6415a6fd06`.
The downloaded artifact archive matched GitHub's published SHA256, and the extracted
installer matched the archive byte for byte.

The remaining acceptance step is testing this installer on the user's Windows/NVIDIA
laptop. No release tag has been created. The local dev server is stopped.

## Product and established decisions

Neurallama is a Windows desktop control panel for llama.cpp's `llama-server`: Tauri 2,
Rust, Svelte 5 runes, TypeScript and Vite. It manages the server, GGUF downloads, engines,
monitoring, VRAM forecasts, auto-tune, benchmarks and client connections, including native
Windows and WSL Hermes profiles. See `docs/SPEC.md` and `docs/CONTRACT.md`.

Target hardware: Windows 11, RTX 5090 Laptop 24 GB, 64 GB RAM. Target models:
Qwen3.6-35B-A3B (MoE) and Qwen3.8-27B (dense) GGUFs.

- Name: **Neurallama**; repo **ooberguts/neurallama**; environment variable
  `NEURALLAMA_HOME`; app data under `AI\neurallama`.
- The user chose a **public** GitHub repository; it is now created and pushed. `gh` is
  authenticated as `ooberguts`. Do not re-ask established product decisions.
- MIT license, Copyright (c) 2026 ooberguts.
- No UI/chart libraries. Restart the server when switching models. Download llama.cpp
  on first run. NSIS installer, per-user, no admin, unsigned.
- reqwest uses native TLS (SChannel on Windows), allowing Mac Windows cross-checks.
- The generated icon is still a placeholder.

## Completed implementation

The previously unfinished UI now builds and type-checks. Connect and Settings tabs are
complete. All **50** frontend command names, registrations and camelCase argument
bindings were compared against Rust and match.

Reviewed the backend seams and fixed:

- Model switching validates the replacement before stopping the current server.
- Auto-tune and benchmarks exclusively own server operations while running; Stop cancels
  them. Failed/cancelled tuning stops an unsaved trial. Tuning saves the winning value
  only after its final load succeeds.
- Benchmarks emit cancellable jobs and expose a Cancel button. Cancellation interrupts
  requests/readiness waits without dropping a process-start operation halfway through.
- Downloads reserve destinations to prevent competing writes to the same partial file.
  Resume/cancel cleanup keeps ownership until safe. Stalled downloads can be cancelled
  or paused promptly; buffered bytes are flushed; malformed resume offsets are rejected.
  Finalization preserves an existing destination instead of deleting it before rename.
- Engine extraction does not replace an existing install and checks cancellation before
  publication. Existing zip-slip protection remains in place.
- Log/status redaction covers `--api-key=value` and advanced extra-argument keys.
- Telemetry and benchmarks use the endpoint captured at launch, so saving a new port/key
  does not redirect them before the server restarts.
- Failed settings saves preserve the previous in-memory settings. Tray enable/disable
  takes effect immediately.
- Removed Svelte runtime assignment warnings. Refreshed engine/settings state after
  install/removal. Prevented benchmark results from modifying another profile's draft.

Regression tests cover overlapping downloads, preserving existing files, cancellation of
a stalled stream, extra-key redaction, failed settings writes and server-operation
ownership. The fake-server lifecycle test also checks failed replacement validation.

The first Windows CI run compiled and linked but its tests failed to start with
`STATUS_ENTRYPOINT_NOT_FOUND`. Commit `5cb03e5` embeds the Common Controls v6 manifest
through the MSVC linker for test executables as well as the application. The next run
passed the Windows tests. See the source comment in `src-tauri/build.rs` for the upstream
Tauri issue and rationale.

## Verification completed

- `npm run build`: passes.
- `npm run check`: **0 errors, 0 warnings**.
- `cargo test`: **89 passed** on macOS.
- `cargo clippy -- -D warnings`: passes.
- `cargo check --target x86_64-pc-windows-msvc`: passes (check-only resource shim).
- `git diff --check`: passes.
- Real Windows CI: dependency install, UI type checks, Rust tests, release build and
  NSIS installer upload all passed. See the run linked above.
- Browser mock: all five tabs inspected at **480×640** and **400×480**, in both light and
  dark. Hermes default-profile first click leaves the config unchanged and requires a
  second click within five seconds. Benchmark cancellation was exercised. Final browser
  console checks reported no warnings/errors after the fixes.

The browser mock supports `?theme=light` and `?theme=dark` in Vite development only;
production follows the OS palette. Port 1420 was occupied by another app, so this session
used `npm run dev -- --host 127.0.0.1 --port 1421`. Its development server is stopped.

## Resume here — target laptop testing

1. Read this file and inspect `git status` and `git log -5 --oneline` before editing.
   The public remote is `origin` at https://github.com/ooberguts/neurallama.git.
2. Download `Neurallama_0.1.0_x64-setup.exe` from the Actions artifact linked above and install it on
   the Windows 11 RTX 5090 laptop. The installer is unsigned and per-user.
3. Exercise SPEC §9: install a CUDA engine, download a GGUF, Serve, then Connect → Test.
4. Compare the VRAM forecast with an actual Qwen3.6-35B-A3B load at 128K/q8 KV. Check
   auto-tune, benchmark and cancellation, crash messages, and app exit killing the child.
5. Verify native/WSL Hermes wiring and backups on actual installed profiles. Do not
   overwrite default profiles without the explicit second click.
6. File/fix hardware findings. A `v0.1.0` tag would draft a GitHub Release; tagging remains
   optional and has not been done.

There is no need to recreate the repository or rerun the first-build setup. A documentation
update after the successful build uses `[skip ci]`; the installer corresponds to code
commit `5cb03e5`.

## Mac development notes

Rust is in `$HOME/.cargo/bin`; use `export PATH="$HOME/.cargo/bin:$PATH"`. Targets already
installed: `aarch64-apple-darwin` and `x86_64-pc-windows-msvc`. Node/npm dependencies are
already installed.

For a Windows cross-check, create this **check-only** `llvm-rc` shim in a scratch directory
outside the repository, make it executable and put it before Cargo on PATH:

```sh
#!/bin/sh
out=""; prev=""
for a in "$@"; do
  case "$prev" in /fo|-fo|/FO|-FO) out="$a";; esac
  case "$a" in /fo*|-fo*) [ ${#a} -gt 3 ] && out="${a#???}";; esac
  prev="$a"
done
[ -n "$out" ] && : > "$out"
exit 0
```

This compiles no Windows resources. The linked CI run verified real Windows compilation/linking.

## Important remaining limits

- Untested on real Windows/NVIDIA: Job Object, CREATE_NO_WINDOW, NVML/nvidia-smi,
  Credential Manager, WSL discovery/push, real engine installs, real server slots,
  auto-tune and benchmarks. README identifies this as early v0.1.
- Fit formulas need comparison with actual loads on the target laptop. Reference:
  MoE Q5 at 128K is tight near `--n-cpu-moe 8` and fits near 10; dense 27B Q4 at 64K
  with q8 KV fits in the estimate.
- llama.cpp `b*` builds are prereleases; stable `v*` releases may point to their promoted
  build through `nightly-tag.txt`. The manager follows that indirection.
- Native Windows Hermes uses `%LOCALAPPDATA%\hermes`; WSL uses `~/.hermes`. Default
  profiles require the UI's second click; there is no select-all. WSL NAT instructions
  show LAN/firewall steps without executing them.
- Nice-to-have: README screenshots, final icon, llama-bench integration and router mode.

Existing implementation commits retain their original co-authorship. The checkpoint
includes the prior UI author's co-author credit and Codex's credit for review fixes.
