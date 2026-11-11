<script lang="ts">
  // Number input bound to `number | null`; empty means null (flag omitted).
  interface Props {
    value: number | null;
    label: string;
    placeholder?: string;
    step?: number | "any";
    min?: number;
    max?: number;
    int?: boolean;
  }
  let { value = $bindable(), label, placeholder = "auto", step = 1, min, max, int = true }: Props = $props();

  function onInput(e: Event & { currentTarget: HTMLInputElement }) {
    const t = e.currentTarget.value.trim();
    if (t === "") {
      value = null;
      return;
    }
    const n = Number(t);
    if (Number.isFinite(n)) value = int ? Math.round(n) : n;
  }
</script>

<label class="fld">
  <span class="flbl">{label}</span>
  <input type="number" {step} {min} {max} {placeholder} value={value ?? ""} oninput={onInput} />
</label>
