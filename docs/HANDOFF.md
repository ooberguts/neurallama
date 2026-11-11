# Neurallama — resume checkpoint (2026-11-11)

The user explicitly asked to pause at a safe stopping point. Local implementation and
verification are complete. **No GitHub repository was created, nothing was pushed, and
Windows CI has not run.** Resume with publishing and Windows CI, after checking this
checkout's status.

## Product and established decisions

Neurallama is a Windows desktop control panel for llama.cpp's `llama-server`: Tauri 2,
Rust, Svelte 5 runes, TypeScript and Vite. It manages the server, GGUF downloads, engines,
monitoring, VRAM forecasts, auto-tune, benchmarks and client connections, including native
Windows and WSL Hermes profiles. See `docs/SPEC.md` and `docs/CONTRACT.md`.

Target hardware: Windows 11, RTX 5090 Laptop 24 GB, 64 GB RAM. Target models:
Qwen3.6-35B-A3B (MoE) and Qwen3.8-27B (dense) GGUFs.

- Name: **Neurallama**; repo **ooberguts/neurallama**; environment variable
  `NEURALLAMA_HOME`; app data under `AI\neurallama`.
- The prior handoff records the user's choice of a **public** GitHub repository. Do not
  re-ask routine product decisions. `gh` is authenticated as `ooberguts`; the repository
  did not exist when checked during this session.
- MIT license, Copyright (c) 2026 ooberguts.
- No UI/chart libraries. Restart the server when switching models. Download llama.cpp
  on first run. NSIS installer, per-user, no admin, unsigned.
- reqwest uses native TLS (SChannel on Windows), allowing Mac Windows cross-checks.
- The generated icon is still a placeholder.

## Completed at this checkpoint

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

## Verification already completed

- `npm run build`: passes.
- `npm run check`: **0 errors, 0 warnings**.
- `cargo test`: **89 passed** on macOS.
- `cargo clippy -- -D warnings`: passes.
- `cargo check --target x86_64-pc-windows-msvc`: passes (check-only resource shim).
- `git diff --check`: passes.
- Browser mock: all five tabs inspected at **480×640** and **400×480**, in both light and
  dark. Hermes default-profile first click leaves the config unchanged and requires a
  second click within five seconds. Benchmark cancellation was exercised. Final browser
  console checks reported no warnings/errors after the fixes.

The browser mock supports `?theme=light` and `?theme=dark` in Vite development only;
production follows the OS palette. Port 1420 was occupied by another app, so this session
used `npm run dev -- --host 127.0.0.1 --port 1421`. Its server is stopped for this pause.

## Resume here

1. Read this file, inspect `git status` and `git log -5 --oneline`. The implementation is
   saved in a local checkpoint commit. Do not discard any newer user changes.
2. Review the committed diff and optionally rerun local checks if the source changed.
3. Create and push the already-planned public repository:

   ```sh
   gh repo create ooberguts/neurallama --public --source . --remote origin --description "Windows control panel for llama.cpp's llama-server: models, VRAM fit calculator, monitoring, Hermes Agent wiring" --push
   ```

4. Watch the first `.github/workflows/build.yml` run on `windows-latest`. It runs `npm ci`,
   UI type checks, Windows Rust tests and `npx tauri build`, then uploads the
   `neurallama-windows-installer` NSIS artifact. Fix actual failures until green. This is
   the first real Windows compile/link; a cross-check is not a substitute.
5. Give the user the installer artifact and the target-machine acceptance checks in
   SPEC §9. Hardware testing remains necessary. Tagging `v0.1.0` to draft a release is
   optional and has not been done.

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

This compiles no Windows resources. Only CI can verify real Windows compilation/linking.

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
