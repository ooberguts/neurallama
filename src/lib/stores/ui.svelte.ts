export type TabId = "monitor" | "models" | "limits" | "connect" | "settings";

class Ui {
  tab = $state<TabId>("monitor");
  /** Profile id or "model:<modelId>" to preselect in the Limits tab. */
  limitsTarget = $state<string | null>(null);

  openLimits(target: string | null) {
    this.limitsTarget = target;
    this.tab = "limits";
  }
}

export const ui = new Ui();
