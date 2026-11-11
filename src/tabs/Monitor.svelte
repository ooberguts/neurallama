<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api } from "../lib/api";
  import { Arm, Busy } from "../lib/busy.svelte";
  import Meter from "../lib/components/Meter.svelte";
  import Sparkline from "../lib/components/Sparkline.svelte";
  import { bytes, compact, date, int, pct, tps } from "../lib/format";
  import { jobs, server, settings, telemetry } from "../lib/stores/app.svelte";
  import { copy, toasts } from "../lib/stores/toasts.svelte";
  import type { InstalledEngine, ModelStats, Release } from "../lib/types";

  const busy = new Busy();
  const arm = new Arm(5000);

  const snap = $derived(telemetry.snap);
  const gpu = $derived(snap?.gpu ?? null);
  const met = $derived(snap?.metrics ?? null);
  const hist = $derived(snap?.history ?? { gpuUtil: [], vramUsed: [], genTps: [] });
  const tpsPeak = $derived(Math.max(0, ...hist.genTps));

  // ---- lifetime stats ----
  let stats = $state<ModelStats[]>([]);
  const loadStats = () => busy.run("stats", async () => (stats = await api.statsList()));

  // ---- engine row ----
  let installed = $state<InstalledEngine[]>([]);
  let releases = $state<Release[]>([]);
  let relErr = $state<string | null>(null);
  let rollbackTag = $state("");
  let updateJob = $state<string | null>(null);

  const active = $derived(installed.find((e) => e.active) ?? null);
  const activeTag = $derived(active?.tag ?? settings.value?.activeEngine ?? null);
  const latest = $derived(releases[0] ?? null);
  const buildNo = (t: string | null | undefined) => Number(/b(\d+)/.exec(t ?? "")?.[1] ?? NaN);
  const newer = $derived(!!latest && !!activeTag && buildNo(latest.tag) > buildNo(activeTag));
  const others = $derived(installed.filter((e) => !e.active));
  const job = $derived(jobs.get(updateJob));

  async function loadEngines() {
    try {
      installed = await api.engineInstalled();
    } catch (e) {
      toasts.error(e);
    }
    try {
      releases = await api.engineReleases();
      relErr = null;
    } catch (e) {
      relErr = e instanceof Error ? e.message : String(e);
    }
  }

  /** Pick the asset in `rel` that matches the active build's flavor (and CUDA version when possible). */
  function matchingAsset(rel: Release) {
    const flavor = active?.flavor ?? "cuda";
    const cands = rel.assets.filter((a) => a.flavor === flavor);
    const cv = /cuda-([\d.]+)/.exec(activeTag ?? "")?.[1];
    return cands.find((a) => cv && a.cudaVersion === cv) ?? cands[0] ?? null;
  }

  async function update() {
    if (!latest) return;
    if (!arm.fire("update")) return;
    const asset = matchingAsset(latest);
    if (!asset) {
      toasts.push("error", `No ${active?.flavor ?? "matching"} build in ${latest.tag}. Pick one in Settings → Engine.`);
      return;
    }
    const rel = latest;
    const flavor = asset.flavor;
    await busy.run("update", async () => {
      const id = await api.engineInstall(rel.tag, asset.name);
      updateJob = id;
      jobs.onFinish(id, (j) => {
        if (j.state !== "done") return;
        busy.run("update", async () => {
          installed = await api.engineInstalled();
          const fresh = installed.find((e) => e.tag.startsWith(rel.tag) && e.flavor === flavor);
          if (!fresh) throw new Error(`Installed ${rel.tag}, but could not find it in the engine list.`);
          settings.set(await api.engineSetActive(fresh.tag));
          installed = await api.engineInstalled();
          if (server.running) server.set(await api.serverRestart());
          toasts.ok(`Now on llama.cpp ${fresh.tag}`);
          updateJob = null;
        });
      });
    });
  }

  async function rollback() {
    if (!rollbackTag) return;
    if (!arm.fire("rollback")) return;
    await busy.run("rollback", async () => {
      settings.set(await api.engineSetActive(rollbackTag));
      installed = await api.engineInstalled();
      if (server.running) server.set(await api.serverRestart());
      toasts.ok(`Rolled back to ${rollbackTag}`);
      rollbackTag = "";
    });
  }

  async function resetStats() {
    if (!arm.fire("reset")) return;
    await busy.run("stats", async () => (stats = await api.statsReset(null)));
  }

  // ---- logs ----
  let filter = $state("");
  let logEl = $state<HTMLPreElement | null>(null);
  let stick = true;
  const shown = $derived.by(() => {
    const f = filter.trim().toLowerCase();
    const src = f ? server.logs.filter((l) => l.toLowerCase().includes(f)) : server.logs;
    return src.slice(-400).join("\n");
  });
  $effect(() => {
    void shown;
    if (stick && logEl) tick().then(() => logEl && (logEl.scrollTop = logEl.scrollHeight));
  });
  function onLogScroll() {
    if (!logEl) return;
    stick = logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight < 24;
  }

  onMount(() => {
    loadStats();
    loadEngines();
    const t = setInterval(loadStats, 15_000);
    return () => clearInterval(t);
  });
</script>

<div class="card col">
  <Meter label="VRAM" used={gpu?.vramUsed} total={gpu?.vramTotal} note={gpu ? `${gpu.name}${gpu.source === "nvidia-smi" ? " · via nvidia-smi" : ""}` : "No NVIDIA GPU detected"} />
  <Meter label="RAM" used={snap?.mem.ramUsed} total={snap?.mem.ramTotal} note={snap?.mem.serverRss != null ? `llama-server working set ${bytes(snap.mem.serverRss)}` : ""} />
</div>

<div class="card graph">
  <div class="gtop">
    <span class="lbl">GPU</span>
    <span class="gsub">{gpu?.tempC != null ? `${Math.round(gpu.tempC)}°C` : ""}{gpu?.powerW != null ? ` · ${Math.round(gpu.powerW)} W` : ""}{gpu?.clockMhz != null ? ` · ${gpu.clockMhz} MHz` : ""}</span>
    <span class="sp"></span>
    <span class="gval">{gpu ? Math.round(gpu.utilPct) : "—"}</span><span class="gsub">%</span>
  </div>
  <Sparkline values={hist.gpuUtil} max={100} color="var(--acc)" label="GPU utilization, last 90 seconds" />
</div>

<div class="card graph">
  <div class="gtop">
    <span class="lbl">Tokens / sec</span>
    <span class="gsub">peak {tps(tpsPeak)}{met?.promptTps ? ` · prompt ${tps(met.promptTps)}` : ""}</span>
    <span class="sp"></span>
    <span class="gval">{tps(met?.genTps ?? hist.genTps[hist.genTps.length - 1])}</span><span class="gsub">tok/s</span>
  </div>
  <Sparkline values={hist.genTps} floor={10} color="var(--ok)" label="Generation tokens per second, last 90 seconds" />
</div>

<div class="tiles">
  <div class="tile"><b>{met?.requestsProcessing ?? "—"}</b><span>Active</span></div>
  <div class="tile"><b>{met?.requestsDeferred ?? "—"}</b><span>Queued</span></div>
  <div class="tile"><b>{met ? `${met.slotsBusy ?? "—"}/${met.slotsTotal ?? "—"}` : "—"}</b><span>Slots</span></div>
  <div class="tile">
    <b class:warn={(met?.kvUsage ?? 0) > 0.85}>{pct(met?.kvUsage)}</b><span>KV fill</span>
  </div>
</div>
{#if met}
  <div class="hint num tokline">
    {compact(met.promptTokensTotal)} prompt · {compact(met.genTokensTotal)} generated since load{met.ctxPerSlot ? ` · ${int(met.ctxPerSlot)} ctx/slot` : ""}
  </div>
{/if}

<div class="secthd">
  <span class="lbl">Lifetime</span><span class="sp"></span>
  <button class="mini" class:armed={arm.is("reset")} disabled={!stats.length || busy.is("stats")} onclick={resetStats}
    >{arm.is("reset") ? "Click again to reset" : "Reset"}</button
  >
</div>
{#if stats.length}
  <div class="card tablewrap">
    <table>
      <thead>
        <tr><th class="l">Model</th><th>Tokens</th><th>Req</th><th>Avg</th><th>Peak</th><th>Low</th></tr>
      </thead>
      <tbody>
        {#each stats as s (s.model)}
          <tr>
            <td class="l ell" title={s.model}>{s.model}</td>
            <td>{compact(s.tokens)}</td>
            <td>{int(s.requests)}</td>
            <td>{tps(s.avgTps)}</td>
            <td class="ok">{tps(s.peakTps)}</td>
            <td>{tps(s.lowTps)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="hint">tok/s, generation</div>
  </div>
{:else}
  <div class="empty">No requests served yet.</div>
{/if}

<div class="secthd"><span class="lbl">Server</span></div>
<div class="card col">
  <div class="row">
    <div class="sdot" class:on={server.status?.state === "ready"}></div>
    <div class="rmeta">
      <div class="rname ell">llama.cpp {met?.build ?? activeTag ?? "not installed"}</div>
      <div class="rsub ell">
        {#if relErr}<span class="warn">Release check failed: {relErr}</span>
        {:else if !latest}checking for updates…
        {:else if newer}<span class="acc">{latest.tag} available</span> · {date(latest.publishedAt)}
        {:else}up to date ({latest.tag}){/if}
      </div>
    </div>
    {#if newer}
      <button
        class="mini"
        class:pri={!arm.is("update")}
        class:armed={arm.is("update")}
        class:busy={busy.is("update")}
        disabled={busy.is("update")}
        onclick={update}>{arm.is("update") ? "Confirm update" : "Update"}</button
      >
    {/if}
  </div>
  {#if arm.is("update")}
    <div class="hint warn">Click again within 5 s. Updating restarts the server, which drops in-flight requests.</div>
  {/if}
  {#if job && updateJob}
    <div class="col tight">
      <div class="row hint"><span class="ell">{job.message ?? job.label}</span><span class="sp"></span><span class="num">{job.total ? pct(job.done / job.total) : ""}</span></div>
      <div class="bar thin"><div class="fill dl" style:width="{job.total ? (job.done / job.total) * 100 : 0}%"></div></div>
    </div>
  {/if}
  {#if others.length}
    <div class="row">
      <select aria-label="Installed build to roll back to" bind:value={rollbackTag}>
        <option value="">Roll back to…</option>
        {#each others as e (e.tag)}<option value={e.tag}>{e.tag}</option>{/each}
      </select>
      <button class="mini" class:armed={arm.is("rollback")} class:busy={busy.is("rollback")} disabled={!rollbackTag || busy.is("rollback")} onclick={rollback}
        >{arm.is("rollback") ? "Confirm" : "Roll back"}</button
      >
    </div>
    {#if arm.is("rollback")}<div class="hint warn">Click again within 5 s. The server restarts on the older build.</div>{/if}
  {/if}
</div>

<div class="secthd">
  <span class="lbl">Log</span><span class="sp"></span>
  <input type="search" class="filter" placeholder="Filter" aria-label="Filter log lines" bind:value={filter} />
  <button class="mini" onclick={() => copy(server.logs.slice(-200).join("\n"), "Copied last 200 log lines")}>Copy last 200 lines</button>
</div>
<pre class="code log" bind:this={logEl} onscroll={onLogScroll} aria-label="Server log" tabindex="0">{shown || (filter ? "No matching lines." : "No log output yet.")}</pre>

<style>
  .graph {
    padding: 8px 10px 6px;
  }
  .gtop {
    display: flex;
    align-items: baseline;
    gap: 6px;
    margin-bottom: 3px;
    min-width: 0;
  }
  .gval {
    font-size: 15px;
    font-weight: 660;
    font-variant-numeric: tabular-nums;
    min-width: 34px;
    text-align: right;
  }
  .gsub {
    font-size: 10px;
    color: var(--faint);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 6px;
  }
  .tile {
    background: var(--card);
    border: 1px solid var(--line);
    border-radius: var(--r);
    padding: 7px 4px;
    text-align: center;
  }
  .tile b {
    display: block;
    font-size: 16px;
    font-weight: 660;
    font-variant-numeric: tabular-nums;
    line-height: 1.2;
  }
  .tile span {
    font-size: 9.5px;
    color: var(--faint);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: 600;
  }
  .tokline {
    margin-top: -3px;
    text-align: center;
  }
  .tablewrap {
    padding: 6px 8px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }
  th {
    font-size: 9.5px;
    color: var(--faint);
    font-weight: 650;
    text-align: right;
    padding: 2px 3px;
    width: 13%;
  }
  th.l {
    width: auto;
  }
  td {
    text-align: right;
    padding: 3px 3px;
    border-top: 1px solid color-mix(in srgb, var(--line) 60%, transparent);
  }
  .l {
    text-align: left;
  }
  .sdot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--faint);
    flex: none;
  }
  .sdot.on {
    background: var(--ok);
  }
  .rmeta {
    flex: 1;
    min-width: 0;
  }
  .rname {
    font-size: 11.5px;
    font-weight: 600;
  }
  .rsub {
    font-size: 10.5px;
    color: var(--faint);
  }
  .acc {
    color: var(--acc);
  }
  .tight {
    gap: 3px;
  }
  .bar.thin {
    height: 4px;
  }
  select {
    flex: 1;
  }
  .filter {
    width: 110px;
    padding: 2px 6px;
  }
  .log {
    height: 170px;
    font-size: 10px;
    line-height: 1.45;
    white-space: pre;
  }
</style>
