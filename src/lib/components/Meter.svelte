<script lang="ts">
  import { bytes } from "../format";

  interface Props {
    label: string;
    used: number | null | undefined;
    total: number | null | undefined;
    /** Fraction at which the bar turns amber / red. */
    warn?: number;
    crit?: number;
    note?: string;
  }
  let { label, used, total, warn = 0.85, crit = 0.95, note = "" }: Props = $props();

  const frac = $derived(used != null && total ? Math.min(1, used / total) : 0);
  const level = $derived(frac >= crit ? "crit" : frac >= warn ? "warn" : "ok");
</script>

<div class="meter">
  <div class="top">
    <span class="lbl">{label}</span>
    <span class="sp"></span>
    <span class="num">{bytes(used)}</span>
    <span class="of">/ {bytes(total, 0)}</span>
    <span class="chip {level}">{total ? Math.round(frac * 100) : "—"}%</span>
  </div>
  <div class="bar" role="meter" aria-label="{label} used" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(frac * 100)}>
    <div class="fill {level}" style:width="{frac * 100}%"></div>
  </div>
  {#if note}<div class="foot">{note}</div>{/if}
</div>

<style>
  .top {
    display: flex;
    align-items: baseline;
    gap: 6px;
    margin-bottom: 5px;
  }
  .num {
    font-size: 15px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .of {
    font-size: 11px;
    color: var(--dim);
    font-variant-numeric: tabular-nums;
  }
  .chip {
    min-width: 40px;
    text-align: center;
  }
  .foot {
    margin-top: 4px;
    font-size: 10px;
    color: var(--faint);
    font-variant-numeric: tabular-nums;
  }
</style>
