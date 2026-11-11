<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { Arm, Busy } from "../lib/busy.svelte";
  import CopyButton from "../lib/components/CopyButton.svelte";
  import Toggle from "../lib/components/Toggle.svelte";
  import { server, settings } from "../lib/stores/app.svelte";
  import type { ClientWriteResult, ConnectInfo, HermesPushResult, HermesTarget, TestResult } from "../lib/types";

  const busy = new Busy();
  const arm = new Arm(5000);

  let info = $state<ConnectInfo | null>(null);
  let reveal = $state(false);
  let snip = $state<"curl" | "python" | "node">("curl");
  let test = $state<TestResult | null>(null);

  const loadInfo = () => busy.run("info", async () => { info = await api.connectInfo(); });

  // Refetch when the settings that shape the URL/key change.
  let lastKey = "";
  $effect(() => {
    const s = settings.value;
    const k = s ? `${s.host}|${s.port}|${s.apiKey}|${s.alias}|${s.lanMode}` : "";
    if (k && k !== lastKey) {
      lastKey = k;
      loadInfo();
    }
  });

  const masked = $derived(info ? (reveal ? info.apiKey : info.apiKey ? `${info.apiKey.slice(0, 3)}${"•".repeat(18)}${info.apiKey.slice(-4)}` : "(none)") : "…");

  async function runTest() {
    test = null;
    await busy.run("test", async () => { test = await api.connectTest(); });
  }

  // ---- LAN mode ----
  async function setLan(on: boolean) {
    if (on && !settings.value?.apiKey.trim()) {
      await busy.run("lan", async () => settings.set(await api.apiKeyRegenerate()));
    }
    await busy.run("lan", async () => {
      await settings.patch({ lanMode: on });
    });
  }

  // ---- Hermes ----
  let targets = $state<HermesTarget[] | null>(null);
  let ctxLen = $state<number | null>(null);
  let pushRes = $state<Record<string, HermesPushResult>>({});
  const loadTargets = () => busy.run("hermes", async () => { targets = await api.hermesTargets(); });

  async function push(t: HermesTarget) {
    if (t.isDefault && !arm.fire(`h:${t.id}`)) return;
    await busy.run(`h:${t.id}`, async () => {
      const r = await api.hermesPush(t.id, ctxLen && ctxLen > 0 ? ctxLen : null);
      pushRes = { ...pushRes, [t.id]: r };
      targets = await api.hermesTargets();
    });
  }

  // ---- Continue ----
  let contRes = $state<ClientWriteResult | null>(null);
  const pushContinue = () => busy.run("cont", async () => { contRes = await api.continuePush(); });

  onMount(() => {
    if (!info) loadInfo();
  });
</script>

<div class="card">
  <div class="kv">
    <span class="kvk">Base URL</span><span class="kvv">{info?.baseUrl ?? "…"}</span>
    <CopyButton text={info?.baseUrl ?? ""} what="base URL" />
  </div>
  <div class="kv">
    <span class="kvk">API key</span><span class="kvv">{masked}</span>
    <button class="mini" onclick={() => (reveal = !reveal)} aria-label={reveal ? "Hide API key" : "Reveal API key"} aria-pressed={reveal}>{reveal ? "Hide" : "Show"}</button>
    <CopyButton text={info?.apiKey ?? ""} what="API key" />
  </div>
  <div class="kv">
    <span class="kvk">Model id</span><span class="kvv">{info?.modelId ?? "…"}</span>
    <CopyButton text={info?.modelId ?? ""} what="model id" />
  </div>
</div>

<div class="card col">
  <div class="row">
    <div class="seg" role="tablist" aria-label="Snippet language">
      <button role="tab" aria-selected={snip === "curl"} onclick={() => (snip = "curl")}>curl</button>
      <button role="tab" aria-selected={snip === "python"} onclick={() => (snip = "python")}>Python</button>
      <button role="tab" aria-selected={snip === "node"} onclick={() => (snip = "node")}>Node</button>
    </div>
    <span class="sp"></span>
    <CopyButton text={info?.snippets[snip] ?? ""} what="{snip} snippet" />
  </div>
  <pre class="code snip">{info?.snippets[snip] ?? "…"}</pre>
  <div class="row">
    <button class="mini pri" class:busy={busy.is("test")} disabled={busy.is("test")} onclick={runTest} title="Sends a real chat completion">Test</button>
    {#if test}
      <span class="chip {test.ok ? 'ok' : 'crit'}">{test.ok ? "OK" : "Failed"}{test.status ? ` ${test.status}` : ""}</span>
      <span class="hint num">{test.latencyMs} ms</span>
    {:else if !server.running}
      <span class="hint">Server is stopped; the test will fail.</span>
    {/if}
  </div>
  {#if test}<div class="reply sel" class:bad={!test.ok}>{test.message}</div>{/if}
</div>

<div class="card col">
  <Toggle
    checked={!!settings.value?.lanMode}
    busy={busy.is("lan")}
    label="LAN mode (bind 0.0.0.0)"
    hint="Other devices on your network can reach the server."
    onchange={setLan}
  />
  {#if settings.value?.lanMode}
    <div class="warnbox">
      Anyone on this network who has the API key can use your GPU. Keep the key private, and only enable this on networks you trust. Windows Firewall may ask to allow llama-server.
      {#if server.running}<b>Restart the server to apply.</b>{/if}
    </div>
    {#each info?.lanUrls ?? [] as u (u)}
      <div class="kv"><span class="kvk">LAN</span><span class="kvv">{u}</span><CopyButton text={u} what="LAN URL" /></div>
    {:else}
      <div class="hint">No LAN addresses found.</div>
    {/each}
  {/if}
</div>

<details class="panel" ontoggle={(e) => e.currentTarget.open && !targets && loadTargets()}>
  <summary>Hermes Agent</summary>
  <div class="body">
    <div class="hint">Writes a named <b>neurallama</b> provider into each config (backup first), clears stale keys from the old provider. Push each target on its own; the default profile asks twice.</div>
    <div class="row">
      <label class="row hint" for="hctx">context_length</label>
      <input id="hctx" type="number" min="0" placeholder="optional" style="width:110px" value={ctxLen ?? ""} oninput={(e) => (ctxLen = e.currentTarget.value === "" ? null : Number(e.currentTarget.value))} />
      <span class="sp"></span>
      <button class="mini" class:busy={busy.is("hermes")} disabled={busy.is("hermes")} onclick={loadTargets} aria-label="Rescan Hermes targets" title="Rescan">↻</button>
    </div>
    {#if !targets}
      <div class="hint">Looking for Hermes installs…</div>
    {:else if !targets.length}
      <div class="empty">No Hermes installs found (native or WSL).</div>
    {:else}
      {#each targets as t (t.id)}
        {@const r = pushRes[t.id]}
        <div class="target" class:here={t.pointsHere}>
          <div class="row">
            <span class="chip {t.kind === 'wsl' ? 'vio' : 'acc'}">{t.kind === "wsl" ? `WSL ${t.distro ?? ""}` : "Native"}</span>
            <b class="ell">{t.profile}</b>
            {#if t.isDefault}<span class="chip">default</span>{/if}
            {#if t.pointsHere}<span class="chip ok">points here</span>{/if}
            <span class="sp"></span>
            <button
              class="mini"
              class:pri={!arm.is(`h:${t.id}`)}
              class:armed={arm.is(`h:${t.id}`)}
              class:busy={busy.is(`h:${t.id}`)}
              disabled={busy.is(`h:${t.id}`)}
              onclick={() => push(t)}
              aria-label="Push to Hermes {t.kind} profile {t.profile}">{arm.is(`h:${t.id}`) ? "Overwrite default?" : "Push"}</button
            >
          </div>
          <div class="mono ell dim sel" title={t.configPath}>{t.configPath}{t.configExists ? "" : " (will be created)"}</div>
          <div class="hint">now: {t.currentProvider ?? "no provider"}{t.currentModel ? ` / ${t.currentModel}` : ""} · would use {t.baseUrl}</div>
          {#if t.note}<div class="hint warn">{t.note}</div>{/if}
          {#if arm.is(`h:${t.id}`)}<div class="hint warn">This is the default profile. Click again within 5 s to change it.</div>{/if}
          {#if r}
            <div class="result" class:bad={!r.ok}>
              <div>{r.message}</div>
              {#if r.backupPath}<div class="mono sel">Backup: {r.backupPath}</div>{/if}
              {#if r.ok}<div><b>Start a new Hermes session to pick it up.</b></div>{/if}
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</details>

<details class="panel">
  <summary>Continue</summary>
  <div class="body">
    <div class="hint">Adds this server as a model in <span class="mono">~/.continue/config.yaml</span> (backup first).</div>
    <div class="row"><button class="mini pri" class:busy={busy.is("cont")} disabled={busy.is("cont")} onclick={pushContinue}>Add to Continue</button></div>
    {#if contRes}
      <div class="result" class:bad={!contRes.ok}>
        <div>{contRes.message}</div>
        <div class="mono sel">{contRes.path}</div>
        {#if contRes.backupPath}<div class="mono sel">Backup: {contRes.backupPath}</div>{/if}
      </div>
    {/if}
  </div>
</details>

<details class="panel">
  <summary>Open WebUI</summary>
  <div class="body">
    <div class="hint">Admin Settings → Connections → OpenAI API → <b>+</b>, then paste:</div>
    <div class="kv"><span class="kvk">URL</span><span class="kvv">{info?.baseUrl}</span><CopyButton text={info?.baseUrl ?? ""} what="base URL" /></div>
    <div class="kv"><span class="kvk">Key</span><span class="kvv">{masked}</span><CopyButton text={info?.apiKey ?? ""} what="API key" /></div>
    <div class="hint">Running Open WebUI in Docker? Use <span class="mono sel">{info?.baseUrl.replace(/\/\/[^/:]+/, "//host.docker.internal")}</span> instead.</div>
  </div>
</details>

<details class="panel">
  <summary>Cline</summary>
  <div class="body">
    <div class="hint">Settings → API Provider: <b>OpenAI Compatible</b>, then:</div>
    <div class="kv"><span class="kvk">Base URL</span><span class="kvv">{info?.baseUrl}</span><CopyButton text={info?.baseUrl ?? ""} what="base URL" /></div>
    <div class="kv"><span class="kvk">API key</span><span class="kvv">{masked}</span><CopyButton text={info?.apiKey ?? ""} what="API key" /></div>
    <div class="kv"><span class="kvk">Model ID</span><span class="kvv">{info?.modelId}</span><CopyButton text={info?.modelId ?? ""} what="model id" /></div>
  </div>
</details>

<details class="panel">
  <summary>Other OpenAI-compatible tools</summary>
  <div class="body">
    <div class="hint">Most tools read these environment variables:</div>
    {#if info}
      {@const env = `OPENAI_BASE_URL=${info.baseUrl}\nOPENAI_API_KEY=${info.apiKey}\nOPENAI_MODEL=${info.modelId}`}
      <div class="row"><span class="sp"></span><CopyButton text={env} what="environment variables" /></div>
      <pre class="code">{reveal ? env : env.replace(info.apiKey, masked)}</pre>
    {/if}
  </div>
</details>

<style>
  .snip {
    height: 132px;
  }
  .reply {
    font-size: 11.5px;
    background: var(--card2);
    border-radius: 7px;
    padding: 6px 8px;
    overflow-wrap: anywhere;
    max-height: 120px;
    overflow-y: auto;
  }
  .warnbox {
    font-size: 11px;
    color: var(--warn);
    background: color-mix(in srgb, var(--warn) 9%, transparent);
    border: 1px solid color-mix(in srgb, var(--warn) 35%, transparent);
    border-radius: 7px;
    padding: 6px 8px;
    line-height: 1.45;
  }
  .target {
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    background: var(--card2);
  }
  .target.here {
    border-color: color-mix(in srgb, var(--ok) 35%, transparent);
  }
  .result {
    font-size: 11px;
    border-left: 3px solid var(--ok);
    padding: 3px 0 3px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-wrap: anywhere;
  }
  .result.bad {
    border-left-color: var(--crit);
  }
</style>
