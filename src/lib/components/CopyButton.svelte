<script lang="ts">
  import { copy } from "../stores/toasts.svelte";
  interface Props {
    text: string;
    what?: string;
    label?: string;
  }
  let { text, what = "value", label = "Copy" }: Props = $props();
</script>

<button class="mini" type="button" aria-label="Copy {what}" title="Copy {what}" onclick={() => copy(text, `Copied ${what}`)}>{label}</button>
