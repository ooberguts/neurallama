<script lang="ts">
  // Tiny fixed-height SVG sparkline. Newest sample at the right edge; the series is
  // right-aligned within `samples` slots so the graph fills in from the right.
  interface Props {
    values: number[];
    samples?: number;
    /** Fixed y max; when omitted the max of the data (min `floor`) is used. */
    max?: number;
    floor?: number;
    color?: string;
    label: string;
  }
  let { values, samples = 90, max, floor = 1, color = "var(--acc)", label }: Props = $props();

  const W = 300;
  const H = 40;

  const top = $derived(max ?? Math.max(floor, ...values) * 1.1);
  const pts = $derived.by(() => {
    const v = values.slice(-samples);
    const off = samples - v.length;
    return v.map((y, i) => {
      const x = ((off + i) / (samples - 1)) * W;
      const yy = H - (Math.max(0, Math.min(top, y)) / top) * (H - 2) - 1;
      return [x, yy] as const;
    });
  });
  const line = $derived(pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join(""));
  const area = $derived(pts.length ? `${line}L${pts[pts.length - 1][0].toFixed(1)},${H}L${pts[0][0].toFixed(1)},${H}Z` : "");
</script>

<svg class="spark" viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label={label}>
  <line x1="0" x2={W} y1={H - 0.5} y2={H - 0.5} class="base" />
  {#if pts.length > 1}
    <path d={area} fill={color} class="area" />
    <path d={line} stroke={color} class="ln" />
  {/if}
</svg>

<style>
  .spark {
    display: block;
    width: 100%;
    height: 40px;
    overflow: visible;
  }
  .ln {
    fill: none;
    stroke-width: 1.6;
    vector-effect: non-scaling-stroke;
    stroke-linejoin: round;
  }
  .area {
    opacity: 0.16;
    stroke: none;
  }
  .base {
    stroke: var(--line);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
</style>
