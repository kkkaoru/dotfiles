// This file runs with bun.
import { EventEmitter } from "node:events";
import type { ClientHttp2Session } from "node:http2";
import { PassThrough } from "node:stream";
import { gzipSync } from "node:zlib";
import { afterEach, expect, it, vi } from "vitest";

import { closePool, postWire } from "../src/transport.ts";

class FakeStream extends PassThrough {
  override end(): this {
    return this;
  }
}

class FakeSession extends EventEmitter {
  closed = false;
  destroyed = false;
  requestCount = 0;
  onTimeout: (() => void) | undefined;
  nextStream: ((stream: FakeStream) => void) | undefined;

  request(): FakeStream {
    this.requestCount += 1;
    const stream = new FakeStream();
    queueMicrotask(() => this.nextStream?.(stream));
    return stream;
  }

  setTimeout(_ms: number, cb: () => void): void {
    this.onTimeout = cb;
  }

  close(): void {
    this.closed = true;
    this.emit("close");
  }

  destroy(): void {
    this.destroyed = true;
    this.emit("close");
  }
}

const ORIGINAL_FETCH = globalThis.fetch;

afterEach(() => {
  closePool();
  globalThis.fetch = ORIGINAL_FETCH;
});

it("pools HTTP/2 sessions, inflates gzip, and closes on idle or shutdown", async () => {
  const session = new FakeSession();
  const dial = (): ClientHttp2Session => session as unknown as ClientHttp2Session;

  session.nextStream = (stream) => {
    stream.emit("response", { ":status": 200 });
    stream.push(Buffer.from("hello "));
    stream.push(Buffer.from("world"));
    stream.push(null);
  };
  const first = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t1",
    body: "{}",
    accept: "application/json",
    dial,
  });
  expect(first.status).toBe(200);
  expect(await first.text()).toBe("hello world");

  session.nextStream = (stream) => {
    stream.emit("response", { ":status": 200, "content-encoding": "gzip" });
    stream.push(gzipSync(Buffer.from('{"ok":true}')));
    stream.push(null);
  };
  const second = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    token: "t1",
    body: "{}",
    accept: "application/json",
    signal: new AbortController().signal,
    dial,
  });
  expect(await second.text()).toBe('{"ok":true}');
  expect(session.requestCount).toBe(2);

  session.emit("goaway");
  session.onTimeout?.();
  expect(session.closed).toBe(true);
  closePool();
});

it("handles close before response, stream error, and aborted signal", async () => {
  const session = new FakeSession();
  const dial = (): ClientHttp2Session => session as unknown as ClientHttp2Session;

  session.nextStream = (stream) => {
    stream.emit("close");
  };
  const empty = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial,
  });
  expect(empty.status).toBe(0);
  expect(await empty.text()).toBe("");

  session.nextStream = (stream) => {
    stream.emit("error", new Error("stream reset"));
  };
  await expect(
    postWire({
      url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
      token: "t",
      body: "{}",
      accept: "application/json",
      dial,
    }),
  ).rejects.toThrow(/stream reset/);

  const controller = new AbortController();
  controller.abort(new Error("cancelled"));
  await expect(
    postWire({
      url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
      token: "t",
      body: "{}",
      accept: "application/json",
      signal: controller.signal,
      dial,
    }),
  ).rejects.toThrow(/cancelled/);
});

it("falls back to fetch when dial throws or session errors before response", async () => {
  const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  globalThis.fetch = (async () =>
    new Response("via-fetch", { status: 201 })) as unknown as typeof fetch;

  const viaThrow = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial: () => {
      throw new Error("no h2");
    },
  });
  expect(viaThrow.status).toBe(201);
  expect(await viaThrow.text()).toBe("via-fetch");
  expect(warnSpy).toHaveBeenCalledTimes(1);

  const session = new FakeSession();
  session.nextStream = () => {
    session.emit("error", new Error("h2 dial error"));
  };
  const viaError = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    signal: new AbortController().signal,
    dial: () => session as unknown as ClientHttp2Session,
  });
  expect(viaError.status).toBe(201);
  expect(await viaError.text()).toBe("via-fetch");
});
