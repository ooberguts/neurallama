<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { Arm, Busy } from "../lib/busy.svelte";
  import { bytes, compact, date, pct } from "../lib/format";
  import { jobs, server, settings } from "../lib/stores/app.svelte";
  import { copy, toasts } from "../lib/stores/toasts.svelte";
  import { ui } from "../lib/stores/ui.svelte";
  import type { FitInput, HfFile, HfModel, LocalModel, Verdict } from "../lib/types";

  const busy = new Busy();
  const arm = new Arm(5000);

  let models = $state<LocalModel[]>([]);
  let diskFree = $state<number | null>(null);
  let fits = $state<Record<string, Verdict | "error">>({});

  const totalSize = $derived(models.reduce((s, m) => s + m.sizeBytes, 0));
  const repoIds = $derived([...new Set(models.filter((m) => m.sourceKnown && m.repoId).map((m) => m.repoId!))]);

  async function scan() {
    await busy.run("scan", async () => {
      const [m, d] = await Promise.all([api.modelsScan(true), api.diskFree().catch(() => null)]);
      models = m;
      diskFree = d;
    });
    computeFits();
  }

  /** fits? badge: fit_estimate with the model's profile (active one first) or defaults. */
  async function computeFits() {
    const s = settings.value;
    const next: Record<string, Verdict | "error"> = {};
    for (const m of models) {
      const prof =
        (s?.activeProfile && s.profiles.find((p) => p.id === s.activeProfile && p.modelPath === m.primaryPath)) ||
        s?.profiles.find((p) => p.modelPath === m.primaryPath);
      const input: FitInput = {
        modelPath: m.primaryPath,
        ctxSize: prof?.ctxSize ?? 32768,
        cacheTypeK: prof?.cacheTypeK ?? "f16",
        cacheTypeV: prof?.cacheTypeV ?? "f16",
        ngl: prof?.ngl ?? 99,
        nCpuMoe: prof?.nCpuMoe ?? null,
        cpuMoeAll: prof?.cpuMoeAll ?? false,
        parallel: prof?.parallel ?? 1,
        vramTotalOverride: null,
        ubatch: prof?.ubatch ?? null,
        flashAttn: prof?.flashAttn ?? true,
      };
      try {
        next[m.id] = (await api.fitEstimate(input)).verdict;
      } catch {
        next[m.id] = "error";
      }
      fits = { ...fits, [m.id]: next[m.id] };
    }
  }

  const fitChip = (v: Verdict | "error" | undefined) =>
    v === "fits" ? { cls: "ok", t: "fits" } : v === "tight" ? { cls: "warn", t: "tight" } : v === "wont_fit" ? { cls: "crit", t: "won't fit" } : v === "error" ? { cls: "", t: "fit ?" } : { cls: "", t: "…" };

  async function serve(m: LocalModel) {
    await busy.run(`serve:${m.id}`, async () => {
      server.set(await api.modelServe(m.id));
      await settings.load();
      toasts.ok(`Serving ${m.name}`);
    });
  }
  async function del(m: LocalModel) {
    if (!arm.fire(`del:${m.id}`)) return;
    await busy.run(`del:${m.id}`, async () => {
      models = await api.modelDelete(m.id);
      toasts.ok(`Deleted ${m.name}`);
    });
  }

  // ---- Get / search ----
  let query = $state("");
  let repos = $state<HfModel[] | null>(null);
  let picked = $state<string | null>(null);
  let files = $state<HfFile[]>([]);
  let groupKey = $state<string | null>(null);
  let mmproj = $state<string>("");

  const ID_RE = /^[\w.-]+\/[\w.-]+$/;

  function get() {
    const ids = query.split(/[\s,]+/).filter(Boolean);
    const good = ids.filter((t) => ID_RE.test(t));
    const bad = ids.filter((t) => !ID_RE.test(t));
    if (!good.length) {
      toasts.push("error", "Enter one or more repo ids like unsloth/Qwen3.8-27B-GGUF, or use Find to search.");
      return;
    }
    if (bad.length) toasts.info(`Skipped: ${bad.join(" ")} (not org/name)`);
    repos = good.map((id) => ({ id, downloads: 0, likes: 0, lastModified: null, gated: false }));
    if (good.length === 1) pick(good[0]);
    else picked = null;
  }
  async function find() {
    const q = query.trim();
    if (!q) return;
    picked = null;
    await busy.run("find", async () => { repos = await api.hfSearch(q); });
  }
  async function pick(id: string) {
    if (picked === id) {
      picked = null;
      return;
    }
    picked = id;
    files = [];
    groupKey = null;
    mmproj = "";
    await busy.run("files", async () => {
      const f = await api.hfFiles(id);
      if (picked !== id) return;
      files = f;
      const mm = f.filter((x) => x.isMmproj);
      mmproj = "";
      if (mm.length) mmproj = (mm.find((x) => /f16/i.test(x.path) && !/bf16/i.test(x.path)) ?? mm[0]).path;
    });
  }

  interface Group {
    key: string;
    quant: string;
    files: HfFile[];
    size: number;
    split: boolean;
  }
  const groups = $derived.by(() => {
    const map = new Map<string, Group>();
    for (const f of files) {
      if (f.isMmproj) continue;
      const key = f.shardGroup ?? f.path;
      const g = map.get(key) ?? { key, quant: f.quant ?? f.path.replace(/\.gguf$/i, ""), files: [], size: 0, split: !!f.shardGroup };
      g.files.push(f);
      g.size += f.size;
      map.set(key, g);
    }
    return [...map.values()].sort((a, b) => a.size - b.size);
  });
  const mmFiles = $derived(files.filter((f) => f.isMmproj));
  const group = $derived(groups.find((g) => g.key === groupKey) ?? null);
  const chosen = $derived(group ? [...group.files.map((f) => f.path), ...(mmproj ? [mmproj] : [])] : []);
  const chosenSize = $derived(
    (group?.size ?? 0) + (mmproj ? (mmFiles.find((f) => f.path === mmproj)?.size ?? 0) : 0),
  );
  const fileUrl = (repo: string, path: string) => `https://huggingface.co/${repo}/resolve/main/${path.split("/").map(encodeURIComponent).join("/")}`;

  async function download() {
    if (!picked || !chosen.length) return;
    const repo = picked;
    await busy.run("dl", async () => {
      await api.downloadStart(repo, chosen);
      toasts.ok(`Downloading ${group?.quant} from ${repo}`);
      groupKey = null;
      picked = null;
    });
  }

  // ---- download queue ----
  let hidden = $state<Record<string, boolean>>({});
  const queue = $derived(
    jobs
      .ofKind("download")
      .filter((j) => !hidden[j.jobId])
      .sort((a, b) => rank(a.state) - rank(b.state)),
  );
  function rank(s: string) {
    return s === "running" ? 0 : s === "queued" ? 1 : s === "paused" ? 2 : 3;
  }
  const doneCount = $derived(jobs.ofKind("download").filter((j) => j.state === "done").length);
  let lastDone = -1;
  $effect(() => {
    const n = doneCount;
    if (lastDone >= 0 && n > lastDone) scan();
    lastDone = n;
  });
  const jobAct = (id: string, fn: () => Promise<unknown>) => busy.run(`job:${id}`, fn);

  onMount(scan);
</script>

{#if queue.length}
  <div class="secthd"><span class="lbl">Downloads</span></div>
  <div class="col tightcol">
    {#each queue as j (j.jobId)}
      {@const frac = j.total ? j.done / j.total : 0}
      {@const live = j.state === "running" || j.state === "queued" || j.state === "paused"}
      <div class="card dl">
        <div class="row">
          <div class="rmeta">
            <div class="rname ell" title={j.label}>{j.label}</div>
            <div class="rsub num ell">
              {bytes(j.done)} / {bytes(j.total)} · {pct(frac)}{j.message ? ` · ${j.message}` : ""}
            </div>
          </div>
          <span class="chip {j.state === 'done' ? 'ok' : j.state === 'error' ? 'crit' : j.state === 'paused' ? 'warn' : j.state === 'running' ? 'acc' : ''}">{j.state}</span>
          {#if j.state === "running" || j.state === "queued"}
            <button class="mini" disabled={busy.is(`job:${j.jobId}`)} onclick={() => jobAct(j.jobId, () => api.downloadPause(j.jobId))} aria-label="Pause download {j.label}">Pause</button>
          {:else if j.state === "paused"}
            <button class="mini" disabled={busy.is(`job:${j.jobId}`)} onclick={() => jobAct(j.jobId, () => api.downloadResume(j.jobId))} aria-label="Resume download {j.label}">Resume</button>
          {/if}
          {#if live}
            <button class="mini danger" disabled={busy.is(`job:${j.jobId}`)} onclick={() => jobAct(j.jobId, () => api.downloadCancel(j.jobId))} aria-label="Cancel download {j.label}">Cancel</button>
          {:else}
            <button class="mini" onclick={() => (hidden[j.jobId] = true)} aria-label="Clear {j.label} from the list">Clear</button>
          {/if}
        </div>
        <div class="bar thin"><div class="fill dl" class:crit={j.state === "error"} style:width="{frac * 100}%"></div></div>
      </div>
    {/each}
  </div>
{/if}

<div class="secthd">
  <span class="lbl">On disk</span>
  <span class="lbl num">{models.length} · {bytes(totalSize)}{diskFree != null ? ` · ${bytes(diskFree, 0)} free` : ""}</span>
  <span class="sp"></span>
  <button class="mini" class:busy={busy.is("scan")} disabled={busy.is("scan")} onclick={scan} aria-label="Rescan models folder" title="Rescan">↻</button>
  <button class="mini" disabled={!repoIds.length} onclick={() => copy(repoIds.join(" "), `Copied ${repoIds.length} repo ids`)} title="Repo ids, space-separated: paste into Get on another PC">Copy ids</button>
  <button class="mini" disabled={!repoIds.length} onclick={() => copy(repoIds.map((r) => `https://huggingface.co/${r}`).join("\n"), `Copied ${repoIds.length} links`)} title="One Hugging Face link per line">Copy links</button>
</div>

{#if models.length}
  <div class="col tightcol">
    {#each models as m (m.id)}
      {@const f = fitChip(fits[m.id])}
      {@const serving = server.status?.modelPath === m.primaryPath && server.running}
      <div class="card model" class:lit={serving}>
        <div class="row">
          <div class="rmeta">
            <div class="rname ell" title={m.primaryPath}>{m.meta?.name ?? m.name}</div>
            <div class="rsub ell">
              {#if m.sourceKnown && m.org}{m.org}{:else}<i>source unknown</i>{/if}
              · <span class="num">{bytes(m.sizeBytes)}</span>{m.files.length > 1 ? ` · ${m.files.length} parts` : ""}
            </div>
          </div>
          <span class="chip {f.cls} fitw" title="Fit with this model's profile (or defaults: 32K, f16 KV, all layers on GPU)">{f.t}</span>
        </div>
        <div class="row badges">
          {#if m.quant}<span class="chip acc">{m.quant}</span>{/if}
          {#if m.meta}<span class="chip">{m.meta.architecture}{m.meta.isMoe ? " · MoE" : ""}</span>{/if}
          {#if m.mmprojPath}<span class="chip vio" title={m.mmprojPath}>mmproj</span>{/if}
          {#if serving}<span class="chip ok">serving</span>{/if}
          <span class="sp"></span>
          <button class="mini" onclick={() => ui.openLimits(`model:${m.id}`)} title="Open in the fit calculator">Limits</button>
          <button class="mini go" class:busy={busy.is(`serve:${m.id}`)} disabled={busy.is(`serve:${m.id}`)} onclick={() => serve(m)} title="Make active and (re)start the server">Serve</button>
          <button class="mini danger" class:armed={arm.is(`del:${m.id}`)} disabled={busy.is(`del:${m.id}`)} onclick={() => del(m)} aria-label="Delete {m.name}"
            >{arm.is(`del:${m.id}`) ? "Sure?" : "Delete"}</button
          >
        </div>
      </div>
    {/each}
  </div>
{:else if !busy.is("scan")}
  <div class="empty">No models yet. Get one below.</div>
{/if}

<div class="card col">
  <span class="lbl">Get a model</span>
  <div class="row">
    <input
      type="text"
      aria-label="Repo ids or search terms"
      placeholder="org/repo-GGUF  (several, space-separated)  or search terms"
      bind:value={query}
      onkeydown={(e) => e.key === "Enter" && (ID_RE.test(query.trim().split(/\s+/)[0] ?? "") ? get() : find())}
      spellcheck="false"
      style="flex:1"
    />
    <button class="mini pri" onclick={get}>Get</button>
    <button class="mini" class:busy={busy.is("find")} disabled={busy.is("find")} onclick={find}>Find</button>
  </div>
  <div class="hint"><b>Get</b> takes exact repo ids (paste a copied list). <b>Find</b> searches Hugging Face for GGUF repos.</div>

  {#if repos}
    {#if !repos.length}
      <div class="empty">No GGUF repos match.</div>
    {:else}
      <div class="res">
        {#each repos as r (r.id)}
          <div class="repo" class:open={picked === r.id}>
            <button class="repohd" onclick={() => pick(r.id)} aria-expanded={picked === r.id}>
              <span class="ell rname">{r.id}</span>
              <span class="sp"></span>
              {#if r.gated}<span class="chip warn">gated</span>{/if}
              {#if r.downloads}<span class="rsub num">↓{compact(r.downloads)} · ♥{compact(r.likes)}{r.lastModified ? ` · ${date(r.lastModified)}` : ""}</span>{/if}
            </button>
            {#if picked === r.id}
              <div class="picker">
                {#if busy.is("files")}
                  <div class="hint">Listing files…</div>
                {:else if !groups.length}
                  <div class="hint">No .gguf files in this repo.</div>
                {:else}
                  <div class="qgrid" role="radiogroup" aria-label="Quant">
                    {#each groups as g (g.key)}
                      <label class="q" class:cur={groupKey === g.key}>
                        <input type="radio" name="quant-{r.id}" value={g.key} bind:group={groupKey} />
                        <span class="qn">{g.quant}</span>
                        <span class="qs num">{bytes(g.size)}{g.split ? ` · ${g.files.length} parts` : ""}</span>
                      </label>
                    {/each}
                  </div>
                  {#if mmFiles.length}
                    <label class="row hint">
                      Vision projector
                      <select bind:value={mmproj} aria-label="Vision projector file">
                        <option value="">none</option>
                        {#each mmFiles as f (f.path)}<option value={f.path}>{f.path} ({bytes(f.size)})</option>{/each}
                      </select>
                    </label>
                  {/if}
                  {#if group}
                    <div class="confirm">
                      <div class="row">
                        <b class="num">{bytes(chosenSize)}</b><span class="hint">in {chosen.length} file{chosen.length > 1 ? "s" : ""}</span>
                        <span class="sp"></span>
                        <button class="mini go" class:busy={busy.is("dl")} disabled={busy.is("dl") || (diskFree != null && chosenSize > diskFree)} onclick={download}>Download</button>
                      </div>
                      {#if diskFree != null && chosenSize > diskFree}<div class="hint bad">Not enough disk space: {bytes(diskFree)} free.</div>{/if}
                      <div class="urls sel">
                        {#each chosen as p (p)}<div class="ell mono" title={fileUrl(r.id, p)}>{fileUrl(r.id, p)}</div>{/each}
                      </div>
                    </div>
                  {:else}
                    <div class="hint">Pick one quant to see the size and source.</div>
                  {/if}
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .tightcol {
    gap: 5px;
  }
  .card.model,
  .card.dl {
    padding: 7px 9px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .card.lit {
    border-color: color-mix(in srgb, var(--ok) 35%, transparent);
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
  .badges {
    flex-wrap: wrap;
    row-gap: 4px;
  }
  .fitw {
    min-width: 64px;
    text-align: center;
  }
  .bar.thin {
    height: 4px;
  }
  .res {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .repo {
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--card2);
    overflow: hidden;
  }
  .repo.open {
    border-color: color-mix(in srgb, var(--acc) 40%, transparent);
  }
  .repohd {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 0;
    background: transparent;
    text-align: left;
    padding: 6px 8px;
    font-weight: 500;
  }
  .picker {
    padding: 4px 8px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .qgrid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(118px, 1fr));
    gap: 4px;
  }
  .q {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: 7px;
    padding: 4px 7px;
    cursor: pointer;
    background: var(--card);
    position: relative;
  }
  .q input {
    position: absolute;
    opacity: 0;
  }
  .q:focus-within {
    outline: 2px solid var(--acc);
  }
  .q.cur {
    border-color: var(--acc);
    background: color-mix(in srgb, var(--acc) 10%, var(--card));
  }
  .qn {
    font-size: 11px;
    font-weight: 650;
  }
  .qs {
    font-size: 10px;
    color: var(--faint);
  }
  .confirm {
    border-top: 1px solid var(--line);
    padding-top: 6px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .urls {
    max-height: 66px;
    overflow-y: auto;
    color: var(--dim);
  }
  label.row select {
    flex: 1;
  }
</style>
