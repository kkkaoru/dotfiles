import type {
  ActiveIdentity,
  DeliveryLease,
  IdentityStore,
  MessageSink,
  RuntimeContext,
} from "./contracts.ts";
import { deliverIncoming, undeliveredInbox } from "./agmsg-operations.ts";
import { combine, errorMessage, uniqueStrings } from "./runtime-helpers.ts";

interface AutomaticCheckRequest {
  readonly activate: (identity: ActiveIdentity) => void;
  readonly context: RuntimeContext;
  readonly ownership: (identity: ActiveIdentity, owned: boolean) => void;
  readonly receive: (identity: ActiveIdentity) => Promise<string>;
  readonly resolveIdentity: () => Promise<ActiveIdentity | undefined>;
  readonly retryQueued: boolean;
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
  #queuedDelivery: string | undefined;
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
      const identity: ActiveIdentity | undefined = await request.resolveIdentity();
      if (identity === undefined) {
        this.#flush(request, this.#identityStore.loadPending(request.context));
        return;
      }
      request.activate(identity);
      const owned: boolean = await this.#lease.claim({ force: false, identity });
      request.ownership(identity, owned);
      await this.#receive(request, identity, owned);
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

  async #receive(
    request: AutomaticCheckRequest,
    identity: ActiveIdentity,
    owned: boolean,
  ): Promise<void> {
    const fetched: string = owned ? await request.receive(identity) : "";
    this.#flush(
      request,
      uniqueStrings(
        [...this.#identityStore.loadPending(request.context), fetched].filter(
          (item: string): boolean => item !== "",
        ),
      ),
    );
  }

  #flush(request: AutomaticCheckRequest, pending: readonly string[]): void {
    const remaining: readonly string[] = undeliveredInbox(
      pending,
      request.context.sessionManager.buildContextEntries?.() ??
        request.context.sessionManager.getEntries(),
    );
    this.#identityStore.savePending(remaining);
    if (remaining.length === 0) {
      this.#queuedDelivery = undefined;
      return;
    }
    const batch: string = combine(remaining);
    if (!request.retryQueued && this.#queuedDelivery === batch) {
      return;
    }
    deliverIncoming(this.#messages, batch);
    this.#queuedDelivery = batch;
  }
}
