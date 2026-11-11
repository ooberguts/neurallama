<script lang="ts">
  import { toasts } from "../stores/toasts.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div class="toast {t.kind}" role={t.kind === "error" ? "alert" : "status"}>
      <span class="msg">{t.text}</span>
      <button class="x" aria-label="Dismiss message" title="Dismiss" onclick={() => toasts.close(t.id)}>×</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 10px;
    right: 10px;
    bottom: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    pointer-events: none;
    z-index: 50;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    background: var(--toast);
    border: 1px solid var(--line);
    border-left: 3px solid var(--acc);
    border-radius: 8px;
    padding: 7px 8px 7px 10px;
    font-size: 11.5px;
    box-shadow: 0 6px 20px #0005;
    backdrop-filter: blur(12px);
    animation: up 0.18s ease-out;
  }
  .toast.error {
    border-left-color: var(--crit);
  }
  .toast.ok {
    border-left-color: var(--ok);
  }
  .msg {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .x {
    padding: 0 6px;
    font-size: 14px;
    line-height: 1.2;
    background: transparent;
    border: 0;
    color: var(--dim);
  }
  @keyframes up {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
