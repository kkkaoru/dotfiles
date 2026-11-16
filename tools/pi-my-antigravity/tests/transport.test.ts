// This file runs with bun.
import { EventEmitter } from "node:events";
import { createServer } from "node:http";
import type { ClientHttp2Session } from "node:http2";
import { PassThrough } from "node:stream";
import { gzipSync } from "node:zlib";
import { afterEach, expect, it, vi } from "vitest";

import { closePool, httpPost, postWire } from "../src/transport.ts";

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

  // Post-settlement error and close on stream/session are handled cleanly.
  session.emit("goaway");
  session.onTimeout?.();
  expect(session.closed).toBe(true);
  closePool();

  // Also test error after response (post-settlement) and immediate close after response.
  const lateSession = new FakeSession();
  lateSession.nextStream = (stream) => {
    stream.emit("response", { ":status": 200 });
    stream.push(null);
    stream.emit("close");
    lateSession.emit("error", new Error("late error"));
  };
  const lateRes = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial: () => lateSession as unknown as ClientHttp2Session,
  });
  expect(lateRes.status).toBe(200);
  // Emit close after response promise has settled to cover the settled guard on stream close.
  lateSession.nextStream = (stream) => {
    stream.emit("response", { ":status": 200 });
    queueMicrotask(() => stream.emit("close"));
  };
  const lateRes2 = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial: () => lateSession as unknown as ClientHttp2Session,
  });
  expect(lateRes2.status).toBe(200);
});

it("re-dials when pooled session is closed and handles missing status", async () => {
  const session1 = new FakeSession();
  const session2 = new FakeSession();
  const calls = { count: 0 };
  const dial = (): ClientHttp2Session => {
    calls.count += 1;
    return (calls.count === 1 ? session1 : session2) as unknown as ClientHttp2Session;
  };

  session1.nextStream = (stream) => {
    stream.emit("response", {});
    stream.push(null);
    stream.emit("close");
  };
  const res1 = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial,
  });
  expect(res1.status).toBe(0);
  session1.close();

  session2.nextStream = (stream) => {
    stream.emit("response", { ":status": 200 });
    stream.push(null);
  };
  const res2 = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial,
  });
  expect(res2.status).toBe(200);
  expect(calls.count).toBe(2);
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
  // Calling closePool while a destroyed session is in POOLS exercises !client.destroyed false branch.
  session.destroyed = true;
  closePool();

  // Destroyed session in pool is replaced on next request.
  const freshSession = new FakeSession();
  freshSession.nextStream = (stream) => {
    stream.emit("response", { ":status": 200 });
    stream.push(null);
  };
  const recovered = await postWire({
    url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
    token: "t",
    body: "{}",
    accept: "application/json",
    dial: () => freshSession as unknown as ClientHttp2Session,
  });
  expect(recovered.status).toBe(200);
  closePool();

  const sessionForString = new FakeSession();
  sessionForString.nextStream = (stream) => {
    stream.emit("error", "raw-reset");
  };
  await expect(
    postWire({
      url: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo",
      token: "t",
      body: "{}",
      accept: "application/json",
      dial: () => sessionForString as unknown as ClientHttp2Session,
    }),
  ).rejects.toThrow(/raw-reset/);

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

it("falls back to HTTP/1.1 when dial throws or session errors before response", async () => {
  const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  const server = createServer((_req, res) => {
    res.writeHead(201, { "content-type": "text/plain" });
    res.end("via-http");
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const address = server.address();
  const port = address && typeof address === "object" ? address.port : 0;

  try {
    const viaThrow = await postWire({
      url: `http://127.0.0.1:${port}/v1internal:fetchUserInfo`,
      token: "t",
      body: "{}",
      accept: "application/json",
      dial: () => {
        throw "non-error-dial";
      },
    });
    expect(viaThrow.status).toBe(201);
    expect(await viaThrow.text()).toBe("via-http");
    expect(warnSpy).toHaveBeenCalledTimes(1);

    const session = new FakeSession();
    session.nextStream = () => {
      session.emit("error", new Error("h2 dial error"));
    };
    const viaError = await postWire({
      url: `http://127.0.0.1:${port}/v1internal:fetchUserInfo`,
      token: "t",
      body: "{}",
      accept: "application/json",
      signal: new AbortController().signal,
      dial: () => session as unknown as ClientHttp2Session,
      fallback: httpPost,
    });
    expect(viaError.status).toBe(201);
    const chunks: Buffer[] = [];
    for await (const chunk of viaError.body) {
      chunks.push(chunk);
    }
    expect(Buffer.concat(chunks).toString("utf8")).toBe("via-http");
  } finally {
    server.close();
  }
});
