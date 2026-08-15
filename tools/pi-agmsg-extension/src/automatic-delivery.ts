import type {
  ActiveIdentity,
  DeliveryLease,
  IdentityStore,
  MessageSink,
  RuntimeContext,
} from "./contracts.ts";
import { deliverIncoming } from "./agmsg-operations.ts";
import { combine, errorMessage } from "./runtime-helpers.ts";

interface AutomaticCheckRequest {
  readonly activate: (identity: ActiveIdentity) => void;
  readonly context: RuntimeContext;
  readonly ownership: (identity: ActiveIdentity, owned: boolean) => void;
  readonly receive: (identity: ActiveIdentity) => Promise<string>;
  readonly resolveIdentity: () => Promise<ActiveIdentity | undefined>;
}

interface AutomaticDeliveryDependencies {
  readonly identityStore: IdentityStore;
  readonly lease: DeliveryLease;
  readonly messages: MessageSink;
}

export class AutomaticDelivery {
  readonly #identityStore: IdentityStore;
  readonly #lease: DeliveryLease;
  readonly #messages: MessageSink;
  #checkAgain = false satisfies boolean;
  #checkFinished: PromiseWithResolvers<boolean> | undefined;
  #checking = false satisfies boolean;
  #enabled = true satisfies boolean;
  #stopped = false satisfies boolean;

  constructor({ identityStore, lease, messages }: AutomaticDeliveryDependencies) {
    this.#messages = messages;
    this.#identityStore = identityStore;
    this.#lease = lease;
  }

  get enabled(): boolean {
    return this.#enabled;
  }

  start(): void {
    this.#stopped = false;
  }

  async stop(): Promise<void> {
    this.#stopped = true;
    await this.#checkFinished?.promise;
    await this.#lease.release();
  }

  async setEnabled(enabled: boolean): Promise<void> {
    this.#enabled = enabled;
    if (!enabled) {
      await this.#lease.release();
    }
  }

  async takeOwnership(identity: ActiveIdentity): Promise<boolean> {
    return this.#lease.claim({ force: true, identity });
  }

  async check(request: AutomaticCheckRequest): Promise<void> {
    if (!this.#enabled || this.#stopped) {
      return;
    }
    if (this.#checking) {
      this.#checkAgain = true;
      return;
    }
    await this.#runCheck(request);
  }

  async #runCheck(request: AutomaticCheckRequest): Promise<void> {
    this.#checking = true;
    const finished: PromiseWithResolvers<boolean> = Promise.withResolvers<boolean>();
    this.#checkFinished = finished;
    try {
      this.#deliverPending(request.context);
      const identity: ActiveIdentity | undefined = await request.resolveIdentity();
      if (identity === undefined) {
        return;
      }
      request.activate(identity);
      const owned: boolean = await this.#lease.claim({ force: false, identity });
      request.ownership(identity, owned);
      if (owned) {
        await this.#receive(request, identity);
      }
    } catch (error: unknown) {
      request.context.ui.setStatus("agmsg", `agmsg: ${errorMessage(error)}`);
    } finally {
      this.#checking = false;
      this.#checkFinished = undefined;
      finished.resolve(true);
      if (this.#checkAgain) {
        this.#checkAgain = false;
        await this.check(request);
      }
    }
  }

  async #receive(request: AutomaticCheckRequest, identity: ActiveIdentity): Promise<void> {
    const output: string = await request.receive(identity);
    if (output === "") {
      return;
    }
    const pending: readonly string[] = [
      ...this.#identityStore.loadPending(request.context),
      output,
    ];
    this.#identityStore.savePending(pending);
    deliverIncoming(this.#messages, combine(pending));
    this.#identityStore.savePending([]);
  }

  #deliverPending(context: RuntimeContext): void {
    const pending: readonly string[] = this.#identityStore.loadPending(context);
    if (pending.length === 0) {
      return;
    }
    deliverIncoming(this.#messages, combine(pending));
    this.#identityStore.savePending([]);
  }
}
