<script lang="ts">
  interface Props {
    checked: boolean;
    label: string;
    hint?: string;
    disabled?: boolean;
    busy?: boolean;
    onchange: (v: boolean) => void;
  }
  let { checked, label, hint = "", disabled = false, busy = false, onchange }: Props = $props();
</script>

<label class="tog" class:on={checked} class:dis={disabled || busy}>
  <input type="checkbox" {checked} disabled={disabled || busy} onchange={(e) => onchange(e.currentTarget.checked)} />
  <span class="sw" aria-hidden="true"></span>
  <span class="txt">
    <span>{label}</span>
    {#if hint}<span class="hint">{hint}</span>{/if}
  </span>
</label>

<style>
  .tog {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    cursor: pointer;
    padding: 4px 0;
  }
  .tog.dis {
    opacity: 0.6;
    cursor: default;
  }
  input {
    position: absolute;
    opacity: 0;
    width: 1px;
    height: 1px;
  }
  .sw {
    width: 30px;
    height: 17px;
    border-radius: 20px;
    background: var(--inset);
    border: 1px solid var(--line);
    position: relative;
    transition: 0.16s;
    flex: none;
    margin-top: 1px;
  }
  .sw::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--faint);
    transition: 0.16s;
  }
  .on .sw {
    background: color-mix(in srgb, var(--ok) 22%, transparent);
    border-color: color-mix(in srgb, var(--ok) 45%, transparent);
  }
  .on .sw::after {
    left: 15px;
    background: var(--ok);
  }
  input:focus-visible + .sw {
    outline: 2px solid var(--acc);
    outline-offset: 2px;
  }
  .txt {
    display: flex;
    flex-direction: column;
    font-size: 12px;
  }
  .txt .hint {
    margin-top: 0;
  }
</style>
