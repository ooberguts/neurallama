<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { Arm, Busy } from "../lib/busy.svelte";
  import OptNum from "../lib/components/OptNum.svelte";
  import { bytes, ctx, date, pct, tps } from "../lib/format";
  import { jobs, server, settings } from "../lib/stores/app.svelte";
  import { copy, toasts } from "../lib/stores/toasts.svelte";
  import { ui } from "../lib/stores/ui.svelte";
  import type { Accuracy, BenchRecord, BenchResult, CommandPreview, FitResult, GpuProcess, LocalModel, Profile, Suggestion } from "../lib/types";

  const busy = new Busy();
  const arm = new Arm(5000);
  const TIP = "sysmem-fallback";

  let models = $state<LocalModel[]>([]);
  let target = $state<string>("");
  let draft = $state<Profile | null>(null);
  let baseline = $state<Profile | null>(null);
  let alias = $state(settings.value?.alias ?? "local");

  const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));
  const profiles = $derived(settings.value?.profiles ?? []);
  const loose = $derived(models.filter((m) => !profiles.some((p) => p.modelPath === m.primaryPath)));
  const model = $derived(models.find((m) => m.primaryPath === draft?.modelPath) ?? null);
  const meta = $derived(model?.meta ?? null);
  const native = $derived(meta?.contextLength ?? 262144);
  const ctxMax = $derived(Math.max(native, draft?.ctxSize ?? 0));
  const dirty = $derived(!!draft && JSON.stringify(draft) !== JSON.stringify(baseline));
  const aliasDirty = $derived(alias.trim() !== (settings.value?.alias ?? ""));
  const isNew = $derived(!baseline);
  const servingThis = $derived(server.running && server.status?.profileId === draft?.id);

  function newProfileFor(m: LocalModel): Profile {
    return {
      id: crypto.randomUUID().replace(/-/g, "").slice(0, 12),
      name: m.meta?.name ? `${m.meta.name} ${m.quant ?? ""}`.trim() : m.name,
      modelPath: m.primaryPath,
      mmprojPath: m.mmprojPath,
      ngl: 99,
      nCpuMoe: null,
      cpuMoeAll: false,
      ctxSize: Math.min(32768, m.meta?.contextLength ?? 32768),
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

  function select(t: string) {
    target = t;
    benchResult = null;
    if (t.startsWith("model:")) {
      const m = models.find((x) => x.id === t.slice(6));
      if (!m) return;
      const existing = profiles.find((p) => p.modelPath === m.primaryPath);
      if (existing) return select(existing.id);
      draft = newProfileFor(m);
      baseline = null;
    } else {
      const p = settings.profile(t);
      if (!p) return;
      draft = clone(p);
      baseline = clone(p);
    }
  }

  function defaultTarget() {
    const s = settings.value;
    if (s?.activeProfile && settings.profile(s.activeProfile)) return s.activeProfile;
    if (profiles[0]) return profiles[0].id;
    if (models[0]) return `model:${models[0].id}`;
    return "";
  }

  // Handoff from other tabs (Models "Limits" button, crash banner).
  $effect(() => {
    const t = ui.limitsTarget;
    if (t && models.length) {
      ui.limitsTarget = null;
      select(t);
    }
  });

  // ---- fit estimate (debounced) ----
  let fit = $state<FitResult | null>(null);
  let fitErr = $state<string | null>(null);
  let fitBusy = $state(false);
  let seq = 0;
  $effect(() => {
    if (!draft || !draft.modelPath) {
      fit = null;
      return;
    }
    const input = {
      modelPath: draft.modelPath,
      ctxSize: draft.ctxSize,
      cacheTypeK: draft.cacheTypeK,
      cacheTypeV: draft.cacheTypeV,
      ngl: draft.ngl,
      nCpuMoe: draft.cpuMoeAll ? null : draft.nCpuMoe,
      cpuMoeAll: draft.cpuMoeAll,
      parallel: Math.max(1, draft.parallel),
      vramTotalOverride: null,
      ubatch: draft.ubatch,
      flashAttn: draft.flashAttn,
    };
    const my = ++seq;
    fitBusy = true;
    const t = setTimeout(async () => {
      try {
        const r = await api.fitEstimate(input);
        if (my === seq) {
          fit = r;
          fitErr = null;
        }
      } catch (e) {
        if (my === seq) fitErr = e instanceof Error ? e.message : String(e);
      } finally {
        if (my === seq) fitBusy = false;
      }
    }, 180);
    return () => clearTimeout(t);
  });

  // ---- command preview (debounced) ----
  let preview = $state<CommandPreview | null>(null);
  let advOpen = $state(false);
  $effect(() => {
    if (!draft || !advOpen) return;
    const snap = $state.snapshot(draft) as Profile;
    const t = setTimeout(async () => {
      try {
        preview = await api.serverCommandPreview(snap);
      } catch (e) {
        preview = { program: "", args: [], display: `# ${e instanceof Error ? e.message : String(e)}` };
      }
    }, 250);
    return () => clearTimeout(t);
  });

  // ---- staging ----
  let procs = $state<GpuProcess[] | null>(null);
  let staged = $state<string[]>([]);
  async function stage(s: Suggestion) {
    if (!draft) return;
    const keys = Object.keys(s.patch ?? {});
    if (keys.length) {
      Object.assign(draft, s.patch);
      staged = [...new Set([...staged, s.label])];
      return;
    }
    if (s.kind === "close_apps") {
      await busy.run("procs", async () => (procs = await api.gpuProcesses()));
    } else if (s.kind === "smaller_quant") {
      toasts.info("Download a smaller quant in Models → Get, then pick it here.");
    }
  }
  function revert() {
    if (baseline) draft = clone(baseline);
    else if (target) select(target);
    staged = [];
  }

  async function save() {
    if (!draft) return;
    const d = $state.snapshot(draft) as Profile;
    if (!d.name.trim()) {
      toasts.push("error", "Give the profile a name.");
      return;
    }
    await busy.run("save", async () => {
      if (aliasDirty) await settings.patch({ alias: alias.trim() || "local" });
      if (dirty) settings.set(await api.profileUpsert(d));
      if (!settings.value?.activeProfile) settings.set(await api.profileSetActive(d.id));
      baseline = clone(d);
      target = d.id;
      staged = [];
      toasts.ok(servingThis ? "Saved. Restart the server to apply." : "Saved to profile.");
    });
  }

  async function setActive() {
    if (!draft) return;
    const id = draft.id;
    await busy.run("active", async () => settings.set(await api.profileSetActive(id)));
  }
  async function delProfile() {
    if (!draft || isNew) return;
    if (!arm.fire("delprof")) return;
    const id = draft.id;
    await busy.run("delprof", async () => {
      settings.set(await api.profileDelete(id));
      select(defaultTarget());
    });
  }
  function duplicate() {
    if (!draft) return;
    draft = { ...clone($state.snapshot(draft) as Profile), id: crypto.randomUUID().replace(/-/g, "").slice(0, 12), name: `${draft.name} (copy)`, bench: null };
    baseline = null;
    target = "";
  }

  // ---- auto-tune ----
  let tuneJob = $state<string | null>(null);
  const tune = $derived(jobs.get(tuneJob));
  const tuning = $derived(!!tune && (tune.state === "running" || tune.state === "queued"));
  async function autotune() {
    if (!draft) return;
    const id = draft.id;
    await busy.run("tune", async () => {
      const j = await api.autotuneStart(id);
      tuneJob = j;
      jobs.onFinish(j, async (p) => {
        if (p.state !== "done") return;
        await settings.load();
        if (target === id) select(id);
        toasts.ok(p.message ?? "Auto-tune finished.");
      });
    });
  }
  const cancelTune = () => tuneJob && busy.run("tunecancel", () => api.autotuneCancel(tuneJob!));

  // ---- benchmark ----
  let benchResult = $state<{ r: BenchResult; prev: BenchRecord | null } | null>(null);
  async function bench() {
    if (!draft) return;
    const id = draft.id;
    const prev = settings.profile(id)?.bench ?? null;
    await busy.run("bench", async () => {
      const r = await api.benchmarkRun(id);
      benchResult = { r, prev };
      const fresh = await api.settingsGet();
      settings.set(fresh);
      const p = fresh.profiles.find((x) => x.id === id);
      if (p && baseline && draft) {
        baseline.bench = p.bench;
        draft.bench = p.bench;
      }
    });
  }
  const delta = (a: number, b: number | undefined) => (b ? `${a >= b ? "+" : ""}${(((a - b) / b) * 100).toFixed(1)}%` : "");

  // ---- tip ----
  const showTip = $derived(!!settings.value && !settings.value.dismissedTips.includes(TIP));
  const dismissTip = () => busy.run("tip", async () => settings.set(await api.tipDismiss(TIP)));

  // ---- stacked bar ----
  const accLabel: Record<Accuracy, string> = { measured: "measured", estimate: "estimate", upper_bound: "upper bound" };
  const accCls: Record<Accuracy, string> = { measured: "ok", estimate: "", upper_bound: "warn" };
  const parts = $derived.by(() => {
    if (!fit) return [];
    return [
      { key: "w", label: "Weights on GPU", v: fit.weightsGpu, color: "var(--acc)", acc: (fit.weightsCpu > 0 ? "estimate" : "measured") as Accuracy, tip: "From the GGUF tensor sizes" },
      { key: "kv", label: "KV cache", v: fit.kvBytes, color: "var(--vio)", acc: fit.kvAccuracy, tip: "" },
      { key: "rs", label: "Recurrent state", v: fit.recurrentBytes, color: "#35c4d8", acc: fit.kvAccuracy, tip: "Fixed per slot for hybrid / SSM layers" },
      { key: "ov", label: "Compute + CUDA", v: fit.computeOverhead, color: "#ff8f3c", acc: fit.overheadAccuracy, tip: "" },
      { key: "dr", label: "Display reserve", v: fit.displayReserve, color: "var(--faint)", acc: "estimate" as Accuracy, tip: "Windows desktop / other apps" },
    ].filter((p) => p.v > 0 || p.key === "kv");
  });
  const scale = $derived(fit ? Math.max(fit.vramTotal, fit.vramNeeded) : 1);
  const verdict = $derived(
    !fit ? null : fit.verdict === "fits" ? { cls: "ok", t: "Fits" } : fit.verdict === "tight" ? { cls: "warn", t: "Tight" } : { cls: "crit", t: "Won't fit" },
  );
  const ramFrac = $derived(fit ? Math.min(1, fit.ramNeeded / Math.max(1, fit.ramTotal)) : 0);

  const presets = [16384, 32768, 65536, 131072];

  async function loadModels() {
    await busy.run("models", async () => (models = await api.modelsScan(true)));
  }

  onMount(async () => {
    await loadModels();
    if (!ui.limitsTarget) select(defaultTarget());
  });
</script>

<div class="row">
  <select aria-label="Profile or model" value={target} onchange={(e) => select(e.currentTarget.value)} style="flex:1">
    {#if isNew && draft}<option value="">{draft.name} (unsaved)</option>{/if}
    {#if profiles.length}
      <optgroup label="Profiles">
        {#each profiles as p (p.id)}
          <option value={p.id}>{p.name}{settings.value?.activeProfile === p.id ? " ★" : ""}</option>
        {/each}
      </optgroup>
    {/if}
    {#if loose.length}
      <optgroup label="Models without a profile">
        {#each loose as m (m.id)}<option value="model:{m.id}">{m.meta?.name ?? m.name} {m.quant ?? ""}</option>{/each}
      </optgroup>
    {/if}
  </select>
  {#if draft && !isNew && settings.value?.activeProfile !== draft.id}
    <button class="mini" disabled={busy.is("active")} onclick={setActive} title="Use this profile on next Start">Make active</button>
  {/if}
</div>

{#if !draft}
  <div class="empty">{busy.is("models") ? "Loading models…" : "No models on disk. Get one in the Models tab."}</div>
{:else}
  <div class="card col">
    <div class="row">
      <span class="lbl">Context</span>
      <span class="sp"></span>
      <b class="num ctxv">{ctx(draft.ctxSize)}</b>
      <span class="hint num">{draft.parallel > 1 ? `${ctx(Math.floor(draft.ctxSize / draft.parallel))}/slot · ` : ""}native {ctx(native)}</span>
    </div>
    <input type="range" min="4096" max={ctxMax} step="4096" bind:value={draft.ctxSize} aria-label="Context size" />
    <div class="seg" role="group" aria-label="Context presets">
      {#each presets as p (p)}
        <button class:cur={draft.ctxSize === p} disabled={p > native} onclick={() => draft && (draft.ctxSize = p)}>{ctx(p)}</button>
      {/each}
      <button class:cur={draft.ctxSize === native} onclick={() => draft && (draft.ctxSize = native)}>native</button>
    </div>

    <div class="grid3">
      <label class="fld"
        ><span class="flbl">KV type K</span>
        <select bind:value={draft.cacheTypeK}><option>f16</option><option>q8_0</option><option>q4_0</option></select></label
      >
      <label class="fld"
        ><span class="flbl">KV type V</span>
        <select bind:value={draft.cacheTypeV}><option>f16</option><option>q8_0</option><option>q4_0</option></select></label
      >
      <label class="fld"
        ><span class="flbl">Parallel slots</span>
        <input type="number" min="1" max="16" bind:value={draft.parallel} /></label
      >
      <label class="fld"
        ><span class="flbl">GPU layers (-ngl)</span>
        <input type="number" min="0" max="999" bind:value={draft.ngl} title="99 = all{meta ? ` (model has ${meta.blockCount})` : ''}" /></label
      >
      <label class="fld"
        ><span class="flbl">--n-cpu-moe</span>
        <input
          type="number"
          min="0"
          max={meta?.blockCount ?? 99}
          placeholder={meta && !meta.isMoe ? "dense" : "off"}
          disabled={draft.cpuMoeAll || (meta != null && !meta.isMoe)}
          value={draft.nCpuMoe ?? ""}
          oninput={(e) => draft && (draft.nCpuMoe = e.currentTarget.value === "" ? null : Math.max(0, Math.round(Number(e.currentTarget.value))))}
        /></label
      >
      <label class="chk" class:dis={meta != null && !meta.isMoe}>
        <input type="checkbox" bind:checked={draft.cpuMoeAll} disabled={meta != null && !meta.isMoe} />
        <span>all experts on CPU<br /><span class="hint">--cpu-moe</span></span>
      </label>
    </div>
    {#if meta}
      <div class="hint num">
        {meta.architecture}{meta.isMoe ? ` · MoE ${meta.expertUsedCount ?? "?"}/${meta.expertCount ?? "?"} experts` : " · dense"} · {meta.blockCount} layers{meta.kvLayers != null && meta.kvLayers !== meta.blockCount ? ` · KV on ${meta.kvLayers}` : ""} · {bytes(model?.sizeBytes)}
      </div>
    {/if}
  </div>

  <div class="card col">
    <div class="row">
      <span class="lbl">VRAM forecast</span>
      {#if fitBusy}<span class="hint">updating…</span>{/if}
      <span class="sp"></span>
      {#if verdict}<span class="chip {verdict.cls} verd">{verdict.t}</span>{/if}
    </div>
    {#if fitErr}
      <div class="hint bad">{fitErr}</div>
    {:else if fit}
      <div class="stackw">
        <div class="stack" role="img" aria-label="VRAM forecast: {bytes(fit.vramNeeded)} of {bytes(fit.vramTotal)}">
          {#each parts as p (p.key)}<i style:width="{(p.v / scale) * 100}%" style:background={p.color} title="{p.label}: {bytes(p.v)}"></i>{/each}
        </div>
        <div class="tk" style:left="calc({(fit.vramTotal / scale) * 100}% - 1px)" title="VRAM total"></div>
      </div>
      <div class="legend">
        {#each parts as p (p.key)}
          <div class="lrow" title={p.tip}>
            <i class="sw" style:background={p.color}></i>
            <span class="ell">{p.label}</span>
            <span class="chip mini {accCls[p.acc]}">{accLabel[p.acc]}</span>
            <span class="sp"></span>
            <b class="num">{bytes(p.v)}</b>
          </div>
        {/each}
        <div class="lrow tot">
          <span>Needed / total</span><span class="sp"></span>
          <b class="num" class:bad={fit.verdict === "wont_fit"} class:warn={fit.verdict === "tight"}>{bytes(fit.vramNeeded)} / {bytes(fit.vramTotal)}</b>
        </div>
        {#if fit.vramFreeNow != null}
          <div class="lrow"><span>Free right now</span><span class="sp"></span><b class="num">{bytes(fit.vramFreeNow)}</b></div>
        {/if}
        {#if fit.measuredVram != null}
          <div class="lrow">
            <span>Measured after load</span><span class="sp"></span>
            <b class="num">{bytes(fit.measuredVram)}{fit.errorPct != null ? ` (forecast ${fit.errorPct > 0 ? "+" : ""}${fit.errorPct.toFixed(1)}%)` : ""}</b>
          </div>
        {/if}
      </div>

      <div class="ram">
        <div class="row"><span class="lbl">RAM</span><span class="hint">weights in RAM {bytes(fit.weightsCpu)}</span><span class="sp"></span><b class="num">{bytes(fit.ramNeeded)} / {bytes(fit.ramTotal, 0)}</b></div>
        <div class="bar"><div class="fill" class:warn={ramFrac > 0.8} class:crit={ramFrac > 0.95} style:width="{ramFrac * 100}%"></div></div>
      </div>

      {#if fit.speedHint}<div class="speed">{fit.speedHint}</div>{/if}
      {#each fit.notes as n, i (i)}<div class="hint">• {n}</div>{/each}

      {#if fit.suggestions.length}
        <div class="col sug">
          <span class="lbl">Suggestions <span class="hint">(stage into the controls; nothing is saved)</span></span>
          <div class="wrap-row">
            {#each fit.suggestions as s, i (i)}
              <button class="mini pri" class:busy={s.kind === "close_apps" && busy.is("procs")} onclick={() => stage(s)}>{s.label}</button>
            {/each}
          </div>
          {#if procs}
            <div class="procs">
              <span class="lbl">Top VRAM users</span>
              {#each procs as p (p.pid)}<div class="lrow"><span class="ell">{p.name} <span class="hint">pid {p.pid}</span></span><span class="sp"></span><b class="num">{bytes(p.vramBytes)}</b></div>{/each}
            </div>
          {/if}
        </div>
      {/if}
    {:else}
      <div class="hint">Calculating…</div>
    {/if}
  </div>

  <div class="card col savebar" class:dirty={dirty || aliasDirty}>
    <div class="row">
      {#if isNew}<span class="chip warn">new profile</span>
      {:else if dirty || aliasDirty}<span class="chip warn">unsaved</span>
      {:else}<span class="chip ok">saved</span>{/if}
      {#if staged.length}<span class="hint ell" title={staged.join(", ")}>staged: {staged.join(", ")}</span>{/if}
      <span class="sp"></span>
      <button class="mini" disabled={!dirty || isNew} onclick={revert}>Revert</button>
      <button class="mini go" class:busy={busy.is("save")} disabled={(!dirty && !aliasDirty) || busy.is("save")} onclick={save}>Save to profile</button>
    </div>
    {#if servingThis && !dirty}<div class="hint">This profile is running. Restart the server to pick up saved changes.</div>{/if}
  </div>

  <div class="card col">
    <div class="row">
      <button class="mini" class:busy={busy.is("tune")} disabled={isNew || dirty || tuning || busy.is("tune") || (meta != null && !meta.isMoe)} onclick={autotune} title={meta && !meta.isMoe ? "Only for MoE models" : "Relaunches the server, measures VRAM and adjusts until ~92% full, then saves"}
        >Auto-tune --n-cpu-moe</button
      >
      <button class="mini" class:busy={busy.is("bench")} disabled={isNew || dirty || busy.is("bench")} onclick={bench} title="Short fixed prompt through the server">Benchmark</button>
      <span class="sp"></span>
      {#if isNew || dirty}<span class="hint">save first</span>{/if}
    </div>
    {#if tune && tuneJob}
      <div class="col tight">
        <div class="row hint">
          <span class="ell">{tune.message ?? tune.label}</span><span class="sp"></span>
          <span class="num">{tune.total ? pct(tune.done / tune.total) : ""}</span>
          {#if tuning}<button class="mini danger" disabled={busy.is("tunecancel")} onclick={cancelTune}>Cancel</button>
          {:else}<span class="chip {tune.state === 'done' ? 'ok' : tune.state === 'error' ? 'crit' : ''}">{tune.state}</span>{/if}
        </div>
        <div class="bar thin"><div class="fill dl" style:width="{tune.total ? (tune.done / tune.total) * 100 : 0}%"></div></div>
      </div>
    {/if}
    {#if busy.is("bench")}<div class="hint">Running benchmark…</div>{/if}
    {#if benchResult}
      {@const r = benchResult.r}
      {@const prev = benchResult.prev}
      <div class="benchgrid num">
        <span></span><span class="lbl">now</span><span class="lbl">previous</span>
        <span class="dim">prompt (pp)</span><b>{tps(r.promptTps)} <small>tok/s</small></b><span>{prev ? `${tps(prev.promptTps)} ${delta(r.promptTps, prev.promptTps)}` : "—"}</span>
        <span class="dim">generate (tg)</span><b>{tps(r.genTps)} <small>tok/s</small></b><span>{prev ? `${tps(prev.genTps)} ${delta(r.genTps, prev.genTps)}` : "—"}</span>
      </div>
      <div class="hint">{r.promptTokens} prompt + {r.genTokens} generated tokens in {(r.totalMs / 1000).toFixed(1)} s{prev ? ` · previous run ${date(prev.at)}` : ""}</div>
    {:else if draft.bench}
      <div class="hint num">Last benchmark ({date(draft.bench.at)}): pp {tps(draft.bench.promptTps)} · tg {tps(draft.bench.genTps)} tok/s</div>
    {/if}
  </div>

  {#if showTip}
    <div class="card tip col">
      <div class="row"><span class="lbl">Tip</span><span class="sp"></span><button class="mini" disabled={busy.is("tip")} onclick={dismissTip} aria-label="Dismiss tip">Got it</button></div>
      <div class="hint tiptext">
        Make VRAM overflow fail fast instead of silently crawling: <b>NVIDIA Control Panel → Manage 3D settings → “CUDA - Sysmem Fallback Policy” → “Prefer No Sysmem Fallback”</b>. Neurallama never changes this for you.
      </div>
    </div>
  {/if}

  <details class="panel" bind:open={advOpen}>
    <summary>Advanced <span class="hint">profile editor & command line</span></summary>
    <div class="body">
      <label class="fld"><span class="flbl">Profile name</span><input type="text" bind:value={draft.name} /></label>
      <label class="fld"
        ><span class="flbl">Model file</span>
        <select bind:value={draft.modelPath}>
          {#if !models.some((m) => m.primaryPath === draft?.modelPath)}<option value={draft.modelPath}>{draft.modelPath || "(none)"}</option>{/if}
          {#each models as m (m.id)}<option value={m.primaryPath}>{m.meta?.name ?? m.name} {m.quant ?? ""}</option>{/each}
        </select></label
      >
      <label class="fld"
        ><span class="flbl">Vision projector (--mmproj)</span>
        <input type="text" placeholder="none" value={draft.mmprojPath ?? ""} oninput={(e) => draft && (draft.mmprojPath = e.currentTarget.value.trim() || null)} spellcheck="false" /></label
      >
      <div class="grid2">
        <label class="chk"><input type="checkbox" bind:checked={draft.flashAttn} /><span>Flash attention <span class="hint">-fa on</span></span></label>
        <label class="chk"><input type="checkbox" bind:checked={draft.jinja} /><span>Jinja template <span class="hint">--jinja</span></span></label>
      </div>
      <div class="grid2">
        <OptNum label="Threads (-t)" bind:value={draft.threads} min={1} />
        <OptNum label="Batch threads (-tb)" bind:value={draft.threadsBatch} min={1} />
        <OptNum label="Batch (-b)" bind:value={draft.batch} min={1} />
        <OptNum label="Micro-batch (-ub)" bind:value={draft.ubatch} min={1} />
      </div>
      <span class="lbl">Sampling defaults</span>
      <div class="grid2">
        <OptNum label="--temp" bind:value={draft.temp} step="any" int={false} min={0} placeholder="model default" />
        <OptNum label="--top-p" bind:value={draft.topP} step="any" int={false} min={0} max={1} placeholder="model default" />
        <OptNum label="--top-k" bind:value={draft.topK} min={0} placeholder="model default" />
        <OptNum label="--min-p" bind:value={draft.minP} step="any" int={false} min={0} max={1} placeholder="model default" />
      </div>
      <label class="fld"><span class="flbl">Extra args (appended verbatim)</span><input type="text" class="mono" placeholder="e.g. --no-mmap --swa-full" bind:value={draft.extraArgs} spellcheck="false" /></label>
      <label class="fld"
        ><span class="flbl">Model id / --alias <span class="hint">(global, all profiles)</span></span>
        <input type="text" bind:value={alias} spellcheck="false" /></label
      >

      <div class="row"><span class="lbl">Command line</span><span class="sp"></span>{#if preview}<button class="mini" onclick={() => preview && copy(preview.display, "Copied command line")}>Copy</button>{/if}</div>
      <pre class="code cmd">{preview?.display ?? "…"}</pre>

      <div class="row">
        <button class="mini" onclick={duplicate}>Duplicate</button>
        <span class="sp"></span>
        {#if !isNew}
          <button class="mini danger" class:armed={arm.is("delprof")} disabled={busy.is("delprof")} onclick={delProfile}>{arm.is("delprof") ? "Click again to delete" : "Delete profile"}</button>
        {/if}
      </div>
    </div>
  </details>
{/if}

<style>
  .ctxv {
    font-size: 15px;
    min-width: 46px;
    text-align: right;
  }
  .chk {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    font-size: 11px;
    padding-top: 13px;
    cursor: pointer;
    line-height: 1.25;
  }
  .grid2 .chk {
    padding-top: 0;
  }
  .chk.dis {
    opacity: 0.5;
  }
  .verd {
    min-width: 70px;
    text-align: center;
    font-size: 10.5px;
  }
  .stackw {
    position: relative;
  }
  .stack {
    display: flex;
    height: 12px;
    border-radius: 5px;
    overflow: hidden;
    background: var(--inset);
  }
  .stack i {
    display: block;
    height: 100%;
    transition: width 0.3s;
    border-right: 1px solid var(--card);
  }
  .tk {
    position: absolute;
    top: -3px;
    bottom: -3px;
    width: 2px;
    border-radius: 2px;
    background: var(--crit);
  }
  .legend {
    display: flex;
    flex-direction: column;
  }
  .lrow {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    padding: 2px 0;
    color: var(--dim);
    min-width: 0;
  }
  .lrow b {
    color: var(--tx);
    font-weight: 600;
    white-space: nowrap;
  }
  .lrow.tot {
    border-top: 1px solid var(--line);
    margin-top: 3px;
    padding-top: 5px;
  }
  .sw {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex: none;
  }
  .chip.mini {
    font-size: 8.5px;
    padding: 1px 5px;
    flex: none;
  }
  .ram {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .speed {
    font-size: 11px;
    color: var(--tx);
    background: var(--card2);
    border-radius: 7px;
    padding: 5px 8px;
  }
  .sug {
    gap: 5px;
    border-top: 1px solid var(--line);
    padding-top: 7px;
  }
  .procs {
    background: var(--card2);
    border-radius: 7px;
    padding: 5px 8px;
  }
  .savebar {
    position: sticky;
    bottom: 0;
    z-index: 2;
    box-shadow: 0 -4px 12px #0003;
  }
  .savebar.dirty {
    border-color: color-mix(in srgb, var(--warn) 40%, transparent);
  }
  .tight {
    gap: 3px;
  }
  .bar.thin {
    height: 4px;
  }
  .benchgrid {
    display: grid;
    grid-template-columns: auto 1fr 1fr;
    gap: 2px 10px;
    font-size: 11.5px;
    align-items: baseline;
  }
  .benchgrid small {
    color: var(--faint);
    font-weight: 400;
  }
  .tip {
    border-color: color-mix(in srgb, var(--acc) 35%, transparent);
  }
  .tiptext b {
    color: var(--tx);
    font-weight: 600;
  }
  .cmd {
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 130px;
  }
</style>
