const GiB = 1024 ** 3;
const MiB = 1024 ** 2;

/** Bytes in human units: GiB above 1 GiB, MiB below. */
export function bytes(n: number | null | undefined, digits = 1): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (Math.abs(n) >= GiB) return `${(n / GiB).toFixed(digits)} GiB`;
  if (Math.abs(n) >= MiB) return `${(n / MiB).toFixed(0)} MiB`;
  if (Math.abs(n) >= 1024) return `${(n / 1024).toFixed(0)} KiB`;
  return `${n} B`;
}

export function gibNum(n: number | null | undefined, digits = 1): string {
  if (n == null || !Number.isFinite(n)) return "—";
  return (n / GiB).toFixed(digits);
}

/** tok/s with one decimal. */
export function tps(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  return n.toFixed(1);
}

export function pct(frac: number | null | undefined, digits = 0): string {
  if (frac == null || !Number.isFinite(frac)) return "—";
  return `${(frac * 100).toFixed(digits)}%`;
}

export function int(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  return Math.round(n).toLocaleString("en-US");
}

/** Compact token counts: 4.8M, 912K. */
export function compact(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(Math.round(n));
}

/** Context length label: 131072 → 128K. */
export function ctx(n: number | null | undefined): string {
  if (n == null) return "—";
  if (n % 1024 === 0) return `${n / 1024}K`;
  return `${(n / 1024).toFixed(1)}K`;
}

export function basename(p: string | null | undefined): string {
  if (!p) return "";
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] ?? p;
}

export function ago(iso: string | null | undefined): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return `${Math.floor(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d`;
}

export function date(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
