<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { Arm, Busy } from "../lib/busy.svelte";
  import CopyButton from "../lib/components/CopyButton.svelte";
  import Toggle from "../lib/components/Toggle.svelte";
  import { bytes, date, pct } from "../lib/format";
  import { jobs, server, settings } from "../lib/stores/app.svelte";
  import { toasts } from "../lib/stores/toasts.svelte";
  import type { AppInfo, DriverInfo, InstalledEngine, Release, Settings } from "../lib/types";

  const busy = new Busy();
  const arm = new Arm(5000);
  const s = $derived(settings.value);

  // ---- network ----
  let port = $state<number>(settings.value?.port ?? 8080);
  let host = $state(settings.value?.host ?? "127.0.0.1");
  let exe = $state(settings.value?.customServerExe ?? "");
  $effect(() => {
    // Keep local edits in sync when settings change elsewhere.
    if (s) {
      port = s.port;
      host = s.host;
      exe = s.customServerExe ?? "";
    }
  });
  const netDirty = $derived(!!s && (port !== s.port || host.trim() !== s.host || (exe.trim() || null) !== (s.customServerExe ?? null)));
  let reveal = $state(false);

  async function saveNet() {
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      toasts.push("error", "Port must be 1–65535.");
      return;
    }
    await busy.run("net", async () => {
      await settings.patch({ port, host: host.trim() || "127.0.0.1", customServerExe: exe.trim() || null });
      toasts.ok(server.running ? "Saved. Restart the server to apply." : "Saved.");
    });
  }
  const nextPort = () => busy.run("port", async () => { port = await api.serverNextFreePort(); });
  async function regen() {
    if (!arm.fire("regen")) return;
    await busy.run("regen", async () => {
      settings.set(await api.apiKeyRegenerate());
      toasts.ok("New API key. Update your clients and restart the server.");
    });
  }

  // ---- toggles ----
  type BoolKey = { [K in keyof Settings]: Settings[K] extends boolean ? K : never }[keyof Settings];
  const toggle = (k: BoolKey) => (v: boolean) => busy.run(k, () => settings.patch({ [k]: v } as Partial<Settings>));

  // ---- HF token ----
  let token = $state("");
  let tokenStored = $state<boolean | null>(null);
  const loadToken = async () => { tokenStored = await api.hfTokenStatus().catch(() => null); };
  async function setToken(v: string | null) {
    await busy.run("tok", async () => {
      await api.hfTokenSet(v);
      token = "";
      await loadToken();
      toasts.ok(v ? "HF token saved to Windows Credential Manager." : "HF token removed.");
    });
  }

  // ---- engine ----
  let installed = $state<InstalledEngine[]>([]);
  let releases = $state<Release[] | null>(null);
  let relErr = $state<string | null>(null);
  let driver = $state<DriverInfo | null>(null);
  let relTag = $state("");
  let flavor = $state<"cuda" | "vulkan" | "cpu">("cuda");
  let assetName = $state("");
  let installJob = $state<string | null>(null);
  const job = $derived(jobs.get(installJob));

  const rel = $derived(releases?.find((r) => r.tag === relTag) ?? null);
  const assets = $derived(rel?.assets.filter((a) => a.flavor === flavor) ?? []);
  const asset = $derived(assets.find((a) => a.name === assetName) ?? null);
  $effect(() => {
    // Default to the highest available CUDA version; show driver compatibility warnings below.
    if (!assets.length) {
      assetName = "";
      return;
    }
    if (assets.some((a) => a.name === assetName)) return;
    // Preselect the highest CUDA version: RTX 50-series (sm_120) needs CUDA 13.x builds.
    const pick = [...assets].sort((a, b) => Number(b.cudaVersion ?? 0) - Number(a.cudaVersion ?? 0))[0];
    assetName = pick.name;
  });
  // Driver warnings are specific to the chosen build's CUDA version.
  let assetDriver = $state<DriverInfo | null>(null);
  $effect(() => {
    const cv = asset?.cudaVersion ?? null;
    if (!cv) {
      assetDriver = null;
      return;
    }
    let live = true;
    api.engineDriverInfo(cv).then(
      (d) => { if (live) assetDriver = d; },
      () => { if (live) assetDriver = null; },
    );
    return () => (live = false);
  });
  const driverTooOld = $derived(
    !assetDriver && !!asset?.cudaVersion && !!driver?.cudaDriverVersion && Number(asset.cudaVersion) > Number(driver.cudaDriverVersion),
  );

  async function loadEngines() {
    await busy.run("eng", async () => { installed = await api.engineInstalled(); });
    driver = await api.engineDriverInfo().catch(() => null);
    await loadReleases();
  }
  async function loadReleases() {
    releases = null;
    try {
      releases = await api.engineReleases();
      relErr = null;
      if (!releases.some((r) => r.tag === relTag)) relTag = releases[0]?.tag ?? "";
    } catch (e) {
      relErr = e instanceof Error ? e.message : String(e);
      releases = [];
    }
  }
  async function setChannel(ch: string) {
    await busy.run("chan", async () => {
      await settings.patch({ engineChannel: ch });
      await loadReleases();
    });
  }
  async function setActive(tag: string) {
    await busy.run(`act:${tag}`, async () => {
      settings.set(await api.engineSetActive(tag));
      installed = await api.engineInstalled();
      toasts.ok(server.running ? `Active build: ${tag}. Restart the server to use it.` : `Active build: ${tag}`);
    });
  }
  async function remove(tag: string) {
    if (!arm.fire(`rm:${tag}`)) return;
    await busy.run(`rm:${tag}`, async () => {
      installed = await api.engineRemove(tag);
      await settings.load();
    });
  }
  async function install() {
    if (!rel || !asset) return;
    const tag = rel.tag;
    await busy.run("inst", async () => {
      const id = await api.engineInstall(tag, asset.name);
      installJob = id;
      jobs.onFinish(id, async (j) => {
        if (j.state === "done") {
          installed = await api.engineInstalled();
          await settings.load();
          toasts.ok(`Installed llama.cpp ${tag}. Set it active below.`);
        }
      });
    });
  }

  let info = $state<AppInfo | null>(null);
  onMount(() => {
    loadToken();
    loadEngines();
    api.appInfo().then((i) => { info = i; }, () => {});
  });
</script>

{#if s}
  <div class="card col">
    <span class="lbl">Install root</span>
    <div class="row">
      <span class="kvv" title={settings.paths?.root}>{settings.paths?.root ?? "…"}</span>
      <button class="mini" disabled={!settings.paths} onclick={() => settings.paths && busy.run("open", () => api.openPath(settings.paths!.root))}>Open folder</button>
    </div>
    <div class="hint">Set <span class="mono">NEURALLAMA_HOME</span> before launch to move it.</div>
  </div>

  <div class="card col">
    <span class="lbl">Server</span>
    <div class="grid2">
      <label class="fld"><span class="flbl">Host</span><input type="text" bind:value={host} spellcheck="false" disabled={s.lanMode} title={s.lanMode ? "LAN mode binds 0.0.0.0 (Connect tab)" : ""} /></label>
      <label class="fld">
        <span class="flbl">Port</span>
        <div class="row">
          <input type="number" min="1" max="65535" bind:value={port} style="flex:1" />
          <button class="mini" class:busy={busy.is("port")} disabled={busy.is("port")} onclick={nextPort} title="Find the next free port">Next free</button>
        </div>
      </label>
    </div>
    <label class="fld"><span class="flbl">Custom llama-server.exe (optional, overrides the managed build)</span><input type="text" bind:value={exe} placeholder="use managed build" spellcheck="false" /></label>
    <div class="hint">If the port is busy, the server won't start; use <b>Next free</b> to pick one.</div>
    <div class="row">
      <span class="sp"></span>
      <button class="mini go" class:busy={busy.is("net")} disabled={!netDirty || busy.is("net")} onclick={saveNet}>Save</button>
    </div>
    <div class="kv">
      <span class="kvk">API key</span>
      <span class="kvv">{reveal ? s.apiKey : `${s.apiKey.slice(0, 3)}${"•".repeat(16)}${s.apiKey.slice(-4)}`}</span>
      <button class="mini" onclick={() => (reveal = !reveal)} aria-label={reveal ? "Hide API key" : "Reveal API key"} aria-pressed={reveal}>{reveal ? "Hide" : "Show"}</button>
      <CopyButton text={s.apiKey} what="API key" />
      <button class="mini" class:armed={arm.is("regen")} class:busy={busy.is("regen")} disabled={busy.is("regen")} onclick={regen}>{arm.is("regen") ? "Sure?" : "Regenerate"}</button>
    </div>
    {#if arm.is("regen")}<div class="hint warn">Clients using the old key stop working. Click again within 5 s.</div>{/if}
  </div>

  <div class="card col tog">
    <span class="lbl">Behavior</span>
    <Toggle checked={s.startWithWindows} busy={busy.is("startWithWindows")} label="Start with Windows" onchange={toggle("startWithWindows")} />
    <Toggle checked={s.autostartServer} busy={busy.is("autostartServer")} label="Start the server when the app opens" onchange={toggle("autostartServer")} />
    <Toggle checked={s.autoRestart} busy={busy.is("autoRestart")} label="Restart the server if it crashes" hint="Stops after 3 crashes in 60 s." onchange={toggle("autoRestart")} />
    <Toggle checked={s.tray} busy={busy.is("tray")} label="Tray icon" onchange={toggle("tray")} />
    <Toggle checked={s.closeToTray} disabled={!s.tray} busy={busy.is("closeToTray")} label="Close to tray" hint="Closing the window keeps the server running." onchange={toggle("closeToTray")} />
    <Toggle checked={s.alwaysOnTop} busy={busy.is("alwaysOnTop")} label="Always on top" onchange={toggle("alwaysOnTop")} />
  </div>

  <div class="card col">
    <div class="row">
      <span class="lbl">Hugging Face token</span><span class="sp"></span>
      {#if tokenStored === null}<span class="chip">unknown</span>
      {:else}<span class="chip {tokenStored ? 'ok' : ''}">{tokenStored ? "stored" : "not set"}</span>{/if}
    </div>
    <div class="row">
      <input type="password" placeholder={tokenStored ? "Replace token…" : "hf_…"} aria-label="Hugging Face token" autocomplete="off" bind:value={token} style="flex:1" />
      <button class="mini pri" class:busy={busy.is("tok")} disabled={!token.trim() || busy.is("tok")} onclick={() => setToken(token.trim())}>Set</button>
      <button class="mini danger" disabled={!tokenStored || busy.is("tok")} onclick={() => setToken(null)}>Clear</button>
    </div>
    <div class="hint">Only needed for gated repos. Stored in Windows Credential Manager, never shown or logged.</div>
  </div>

  <div class="card col">
    <div class="row">
      <span class="lbl">Engine (llama.cpp)</span><span class="sp"></span>
      <div class="seg" role="radiogroup" aria-label="Release channel">
        <button role="radio" aria-checked={s.engineChannel === "stable"} class:cur={s.engineChannel === "stable"} disabled={busy.is("chan")} onclick={() => setChannel("stable")}>Stable</button>
        <button role="radio" aria-checked={s.engineChannel === "prerelease"} class:cur={s.engineChannel === "prerelease"} disabled={busy.is("chan")} onclick={() => setChannel("prerelease")}>Prerelease</button>
      </div>
    </div>

    {#if driver}
      <div class="drv">
        <div class="hint num">{driver.gpuName ?? "No NVIDIA GPU"}{driver.driverVersion ? ` · driver ${driver.driverVersion}` : ""}{driver.cudaDriverVersion ? ` · CUDA ≤ ${driver.cudaDriverVersion}` : ""}{driver.computeCap ? ` · sm ${driver.computeCap}` : ""}</div>
        {#each driver.warnings as w, i (i)}<div class="hint warn">⚠ {w}</div>{/each}
      </div>
    {/if}

    <span class="flbl">Installed</span>
    {#if installed.length}
      {#each installed as e (e.tag)}
        <div class="eng" class:act={e.active}>
          <span class="ell mono" title={e.dir}>{e.tag}</span>
          <span class="chip">{e.flavor}</span>
          {#if e.active}<span class="chip ok">active</span>{/if}
          <span class="sp"></span>
          {#if !e.active}
            <button class="mini" class:busy={busy.is(`act:${e.tag}`)} disabled={busy.is(`act:${e.tag}`)} onclick={() => setActive(e.tag)}>Set active</button>
          {/if}
          <button class="mini danger" class:armed={arm.is(`rm:${e.tag}`)} disabled={busy.is(`rm:${e.tag}`)} onclick={() => remove(e.tag)} aria-label="Remove build {e.tag}">{arm.is(`rm:${e.tag}`) ? "Sure?" : "Remove"}</button>
        </div>
      {/each}
    {:else if !busy.is("eng")}
      <div class="empty">No llama.cpp build installed. Install one below.</div>
    {/if}

    <span class="flbl">Available</span>
    {#if relErr}
      <div class="hint bad">{relErr}</div>
    {:else if !releases}
      <div class="hint">Checking GitHub releases…</div>
    {:else if releases.length}
      <div class="row">
        <select aria-label="Release" bind:value={relTag} style="flex:1">
          {#each releases as r (r.tag)}<option value={r.tag}>{r.stableTag ? `${r.stableTag} (${r.tag})` : r.tag} · {date(r.publishedAt)}{r.prerelease ? " · pre" : ""}</option>{/each}
        </select>
        <div class="seg" role="radiogroup" aria-label="Build flavor">
          <button role="radio" aria-checked={flavor === "cuda"} class:cur={flavor === "cuda"} onclick={() => (flavor = "cuda")}>CUDA</button>
          <button role="radio" aria-checked={flavor === "vulkan"} class:cur={flavor === "vulkan"} onclick={() => (flavor = "vulkan")}>Vulkan</button>
          <button role="radio" aria-checked={flavor === "cpu"} class:cur={flavor === "cpu"} onclick={() => (flavor = "cpu")}>CPU</button>
        </div>
      </div>
      {#if assets.length > 1}
        <select aria-label="Asset" bind:value={assetName}>
          {#each assets as a (a.name)}<option value={a.name}>{a.cudaVersion ? `CUDA ${a.cudaVersion}` : a.name}</option>{/each}
        </select>
      {/if}
      {#if asset}
        <div class="asset">
          <div class="mono ell sel" title={asset.name}>{asset.name}</div>
          <div class="hint num">{bytes(asset.size)}{asset.cudartSize ? ` + cudart ${bytes(asset.cudartSize)} = ${bytes(asset.size + asset.cudartSize)}` : ""}</div>
          <div class="mono ell dim sel" title={asset.url}>{asset.url}</div>
          {#if asset.cudartUrl}<div class="mono ell dim sel" title={asset.cudartUrl}>{asset.cudartUrl}</div>{/if}
          {#each assetDriver?.warnings ?? [] as w, i (i)}<div class="hint warn">⚠ {w}</div>{/each}
          {#if driverTooOld}<div class="hint warn">This build needs CUDA {asset.cudaVersion}; your driver supports up to {driver?.cudaDriverVersion}. Update the NVIDIA driver or pick an older CUDA build.</div>{/if}
          <div class="row">
            {#if rel}<a class="hint" href={rel.htmlUrl} target="_blank" rel="noreferrer">Release notes</a>{/if}
            <span class="sp"></span>
            <button class="mini go" class:busy={busy.is("inst")} disabled={busy.is("inst") || (!!job && job.state === "running")} onclick={install}>Install</button>
          </div>
        </div>
      {:else}
        <div class="hint">No {flavor} build in this release.</div>
      {/if}
      {#if job && installJob}
        <div class="row hint">
          <span class="ell">{job.label}: {job.message ?? job.state}</span><span class="sp"></span><span class="num">{job.total ? pct(job.done / job.total) : ""}</span>
          {#if job.state === "running" || job.state === "queued"}
            <button class="mini danger" disabled={busy.is("instcancel")} onclick={() => installJob && busy.run("instcancel", () => api.downloadCancel(installJob!))} aria-label="Cancel engine install">Cancel</button>
          {/if}
        </div>
        <div class="bar thin"><div class="fill dl" class:crit={job.state === "error"} style:width="{job.total ? (job.done / job.total) * 100 : 0}%"></div></div>
      {/if}
    {:else}
      <div class="hint">No releases found.</div>
    {/if}
  </div>

  <div class="hint foot">
    No telemetry. Network calls: GitHub releases, Hugging Face, and your local server only.
    {#if info}<br />Neurallama {info.version} · {info.os}{/if}
  </div>
{/if}

<style>
  .kvv {
    flex: 1;
  }
  .tog {
    gap: 2px;
  }
  .eng {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px;
    border-radius: 7px;
    background: var(--card2);
    min-width: 0;
  }
  .eng.act {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--ok) 35%, transparent);
  }
  .asset {
    background: var(--card2);
    border-radius: 7px;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .seg button {
    font-size: 10.5px;
  }
  .bar.thin {
    height: 4px;
  }
  a {
    color: var(--acc);
  }
  .foot {
    text-align: center;
    padding: 4px 0 2px;
  }
</style>
