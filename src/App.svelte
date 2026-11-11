<script lang="ts">
  import { onMount } from "svelte";
  import { api, isTauri } from "./lib/api";
  import { Busy } from "./lib/busy.svelte";
  import { initStores, server, settings } from "./lib/stores/app.svelte";
  import { ui, type TabId } from "./lib/stores/ui.svelte";
  import { basename, ago } from "./lib/format";
  import Toasts from "./lib/components/Toasts.svelte";
  import Monitor from "./tabs/Monitor.svelte";
  import Models from "./tabs/Models.svelte";
  import Limits from "./tabs/Limits.svelte";
  import Connect from "./tabs/Connect.svelte";
  import SettingsTab from "./tabs/Settings.svelte";

  const tabs: { id: TabId; label: string }[] = [
    { id: "monitor", label: "Monitor" },
    { id: "models", label: "Models" },
    { id: "limits", label: "Limits" },
    { id: "connect", label: "Connect" },
    { id: "settings", label: "Settings" },
  ];

  // Tabs mount on first visit and stay mounted, so drafts survive tab switches.
  let visited = $state<Record<string, boolean>>({ monitor: true });
  // Each tab keeps its own scroll position in the shared scroller (pre: read before the DOM swaps panes).
  let mainEl = $state<HTMLElement | null>(null);
  const scrollPos: Record<string, number> = {};
  let shownTab: TabId = "monitor";
  $effect.pre(() => {
    const t = ui.tab;
    visited[t] = true;
    if (!mainEl || t === shownTab) return;
    scrollPos[shownTab] = mainEl.scrollTop;
    shownTab = t;
    const el = mainEl;
    requestAnimationFrame(() => (el.scrollTop = scrollPos[t] ?? 0));
  });

  const busy = new Busy();
  let ready = $state(false);
  let now = $state(Date.now());

  onMount(() => {
    initStores().finally(() => (ready = true));
    const t = setInterval(() => (now = Date.now()), 15_000);
    return () => clearInterval(t);
  });

  const st = $derived(server.status);
  const srvState = $derived(st?.state ?? "stopped");
  const pill = $derived.by(() => {
    switch (srvState) {
      case "ready":
        return { cls: "ok", text: "Ready" };
      case "attached":
        return { cls: "ok", text: "Attached" };
      case "loading":
        return { cls: "warn", text: st?.loadProgress != null ? `Loading ${Math.round(st.loadProgress * 100)}%` : "Loading" };
      case "starting":
        return { cls: "warn", text: "Starting" };
      case "crashed":
        return { cls: "crit", text: "Crashed" };
      default:
        return { cls: "", text: "Stopped" };
    }
  });
  const sub = $derived.by(() => {
    void now;
    if (!st) return "connecting…";
    const prof = settings.profile(st.profileId);
    const model = prof?.name ?? basename(st.modelPath);
    if (srvState === "stopped") return settings.active ? `Next: ${settings.active.name}` : "No profile selected";
    const up = st.startedAt && (srvState === "ready" || srvState === "attached") ? ` · up ${ago(st.startedAt)}` : "";
    return `${model || "—"} · :${st.port}${up}`;
  });

  const start = () => busy.run("srv", async () => server.set(await api.serverStart(null)));
  const stop = () => busy.run("srv", async () => server.set(await api.serverStop()));
  const restart = () => busy.run("srv", async () => server.set(await api.serverRestart()));

  function onTabKey(e: KeyboardEvent) {
    const i = tabs.findIndex((t) => t.id === ui.tab);
    let n = -1;
    if (e.key === "ArrowRight") n = (i + 1) % tabs.length;
    else if (e.key === "ArrowLeft") n = (i - 1 + tabs.length) % tabs.length;
    if (n >= 0) {
      e.preventDefault();
      ui.tab = tabs[n].id;
      document.getElementById(`tab-${tabs[n].id}`)?.focus();
    }
  }
</script>

<div class="shell">
  <header>
    <div class="dot {pill.cls}" class:pulse={srvState === "ready"} aria-hidden="true"></div>
    <div class="brandbox">
      <div class="brand">Neura<span>Llama</span>{#if !isTauri}<span class="chip vio demo" title="Plain browser: showing mock data">demo</span>{/if}</div>
      <div class="state ell" title={sub}>{sub}</div>
    </div>
    <span class="sp"></span>
    <span class="chip {pill.cls} pillw" role="status" aria-label="Server status: {pill.text}">{pill.text}</span>
    {#if server.running}
      <button class="mini" class:busy={busy.is("srv")} disabled={busy.is("srv")} onclick={restart} title="Restart llama-server">Restart</button>
      <button class="mini stop" disabled={busy.is("srv")} onclick={stop} title="Stop llama-server">Stop</button>
    {:else}
      <button class="mini go" class:busy={busy.is("srv")} disabled={busy.is("srv") || !ready} onclick={start} title="Start llama-server with the active profile">Start</button>
    {/if}
  </header>

  {#if srvState === "crashed" && st?.lastError}
    <div class="crash" role="alert">
      <div class="ctext">
        <b>Server stopped unexpectedly{st.crashCount > 1 ? ` (${st.crashCount}× in 60 s)` : ""}.</b>
        <span class="sel">{st.lastError}</span>
        {#if st.crashCount >= 3}<span class="hint">Auto-restart paused after 3 crashes in 60 s.</span>{/if}
      </div>
      <button class="mini" onclick={() => ui.openLimits(st?.profileId ?? null)}>Open in Limits</button>
    </div>
  {/if}

  <div class="tabs" role="tablist" aria-label="Sections" tabindex="-1" onkeydown={onTabKey}>
    {#each tabs as t (t.id)}
      <button
        id="tab-{t.id}"
        role="tab"
        class="tab"
        aria-selected={ui.tab === t.id}
        aria-controls="pane-{t.id}"
        tabindex={ui.tab === t.id ? 0 : -1}
        onclick={() => (ui.tab = t.id)}>{t.label}</button
      >
    {/each}
  </div>

  <main bind:this={mainEl}>
    {#if ready}
      {#each tabs as t (t.id)}
        {#if visited[t.id]}
          <div id="pane-{t.id}" role="tabpanel" aria-labelledby="tab-{t.id}" hidden={ui.tab !== t.id} class="pane">
            {#if t.id === "monitor"}<Monitor />
            {:else if t.id === "models"}<Models />
            {:else if t.id === "limits"}<Limits />
            {:else if t.id === "connect"}<Connect />
            {:else}<SettingsTab />{/if}
          </div>
        {/if}
      {/each}
    {:else}
      <div class="empty">Loading…</div>
    {/if}
  </main>
</div>

<Toasts />

<style>
  .shell {
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 10px 10px 0;
    gap: 8px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: none;
    min-width: 0;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--faint);
    flex: none;
  }
  .dot.ok {
    background: var(--ok);
  }
  .dot.warn {
    background: var(--warn);
  }
  .dot.crit {
    background: var(--crit);
  }
  .dot.pulse {
    animation: pulse 2.4s infinite;
  }
  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 color-mix(in srgb, var(--ok) 50%, transparent);
    }
    70% {
      box-shadow: 0 0 0 7px transparent;
    }
  }
  .brandbox {
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
  }
  .brand {
    font-weight: 700;
    font-size: 14px;
    letter-spacing: 0.2px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .brand span:not(.chip) {
    color: var(--acc);
    margin-left: -6px;
  }
  .demo {
    font-size: 8.5px;
  }
  .state {
    font-size: 10.5px;
    color: var(--dim);
  }
  .pillw {
    min-width: 62px;
    text-align: center;
  }
  .crash {
    flex: none;
    display: flex;
    gap: 8px;
    align-items: flex-start;
    background: color-mix(in srgb, var(--crit) 10%, var(--card));
    border: 1px solid color-mix(in srgb, var(--crit) 35%, transparent);
    border-radius: var(--r);
    padding: 7px 9px;
    font-size: 11px;
  }
  .ctext {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-wrap: anywhere;
  }
  .tabs {
    flex: none;
    display: flex;
    gap: 2px;
    background: var(--card);
    border: 1px solid var(--line);
    border-radius: 9px;
    padding: 3px;
  }
  .tab {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--dim);
    font-size: 11px;
    font-weight: 650;
    padding: 5px 0;
    border-radius: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tab:hover:not([aria-selected="true"]) {
    color: var(--tx);
    background: transparent;
  }
  .tab[aria-selected="true"] {
    background: var(--card2);
    color: var(--tx);
    box-shadow: 0 1px 3px #0004;
  }
  main {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    margin: 0 -10px;
    padding: 0 10px 12px;
    scrollbar-gutter: stable;
  }
  .pane {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .pane[hidden] {
    display: none;
  }
</style>
