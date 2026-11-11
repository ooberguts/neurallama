import { toasts } from "./stores/toasts.svelte";

/**
 * Per-key busy flags for async actions. `run` never throws: errors become toasts
 * carrying the backend's own message, and the result is undefined.
 */
export class Busy {
  map = $state<Record<string, boolean>>({});

  is(key: string): boolean {
    return !!this.map[key];
  }
  get any(): boolean {
    return Object.values(this.map).some(Boolean);
  }
  async run<T>(key: string, fn: () => Promise<T>): Promise<T | undefined> {
    if (this.map[key]) return undefined;
    this.map[key] = true;
    try {
      return await fn();
    } catch (e) {
      toasts.error(e);
      return undefined;
    } finally {
      this.map[key] = false;
    }
  }
}

/**
 * Two-click confirmation: the first `arm(key)` arms for `ms`, a second call within that
 * window returns true. Used for Delete, Update, pushing to a Hermes default profile, etc.
 */
export class Arm {
  armed = $state<string | null>(null);
  #timer: ReturnType<typeof setTimeout> | null = null;
  #ms: number;
  constructor(ms = 5000) {
    this.#ms = ms;
  }

  is(key: string): boolean {
    return this.armed === key;
  }
  /** Returns true when this click should go ahead. */
  fire(key: string): boolean {
    if (this.armed === key) {
      this.reset();
      return true;
    }
    this.armed = key;
    if (this.#timer) clearTimeout(this.#timer);
    this.#timer = setTimeout(() => (this.armed = null), this.#ms);
    return false;
  }
  reset() {
    this.armed = null;
    if (this.#timer) clearTimeout(this.#timer);
    this.#timer = null;
  }
}
