// This TypeScript file is executed with Bun.
const SETTLED_DELIVERY_DELAY_MS = 0;
type DeferredTask = ReturnType<typeof globalThis.setTimeout>;

export class SettledDelivery {
  #task: DeferredTask | undefined;

  schedule(callback: () => void): void {
    if (this.#task !== undefined) {
      return;
    }
    this.#task = globalThis.setTimeout((): void => {
      this.#task = undefined;
      callback();
    }, SETTLED_DELIVERY_DELAY_MS);
  }

  cancel(): void {
    if (this.#task === undefined) {
      return;
    }
    globalThis.clearTimeout(this.#task);
    this.#task = undefined;
  }
}
