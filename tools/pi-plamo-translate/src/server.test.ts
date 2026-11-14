// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import {
  commandSpec,
  completionText,
  createServer,
  isRefused,
  SETUP_HINT,
  type Child,
  type ResponseLike,
  type ServerDeps,
} from "./server.ts";

function response(ok: boolean, body: unknown, status = 200): ResponseLike {
  return {
    ok,
    status,
    json: () => Promise.resolve(body),
  };
}

test("mlx is preferred when its server binary exists", () => {
  expect(commandSpec("/Users/x", "darwin", 18555, () => true)).toStrictEqual({
    command: "/Users/x/.local/bin/mlx_lm.server",
    args: [
      "--model",
      "/Users/x/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit",
      "--port",
      "18555",
      "--trust-remote-code",
    ],
  });
});

test("llama.cpp is used when mlx is absent", () => {
  expect(
    commandSpec(
      "/Users/x",
      "linux",
      18555,
      (file) => file === "/Users/x/.local/share/plamo-translate/llama.cpp/llama-server",
    ),
  ).toStrictEqual({
    command: "/Users/x/.local/share/plamo-translate/llama.cpp/llama-server",
    args: [
      "-m",
      "/Users/x/.local/share/plamo-translate/plamo-3-translate-0.8b-Q4_K_M.gguf",
      "-c",
      "16384",
      "--port",
      "18555",
    ],
  });
});

test("Windows looks for llama-server.exe", () => {
  expect(
    commandSpec(
      "/Users/x",
      "win32",
      18555,
      (file) => file === "/Users/x/.local/share/plamo-translate/llama.cpp/llama-server.exe",
    ),
  ).toStrictEqual({
    command: "/Users/x/.local/share/plamo-translate/llama.cpp/llama-server.exe",
    args: [
      "-m",
      "/Users/x/.local/share/plamo-translate/plamo-3-translate-0.8b-Q4_K_M.gguf",
      "-c",
      "16384",
      "--port",
      "18555",
    ],
  });
});

test("a missing server binary is reported", () => {
  expect(commandSpec("/Users/x", "darwin", 18555, () => false)).toBeUndefined();
});

test("a refused connection is recognized through its cause and aggregate errors", () => {
  expect(isRefused({ code: "ECONNREFUSED" })).toBe(true);
  expect(isRefused({ cause: { code: "ECONNREFUSED" } })).toBe(true);
  expect(isRefused({ errors: [{ code: "ECONNREFUSED" }] })).toBe(true);
  expect(isRefused(new Error("other"))).toBe(false);
  expect(isRefused("nope")).toBe(false);
});

test("completion text is read only from the first choice", () => {
  expect(completionText({ choices: [{ text: "こんにちは" }] })).toBe("こんにちは");
  expect(completionText({ choices: [] })).toBeNull();
  expect(completionText({ choices: [{ text: 1 }] })).toBeNull();
  expect(completionText({})).toBeNull();
  expect(completionText("nope")).toBeNull();
});

test("a healthy server is not started", async () => {
  const spawned: string[] = [];
  const server = createServer(
    {
      exists: () => true,
      fetch: () => Promise.resolve(response(true, {})),
      home: "/Users/x",
      now: () => 0,
      platform: "darwin",
      report: () => undefined,
      sleep: () => Promise.resolve(),
      spawn: (command) => {
        spawned.push(command);
        return { once: () => undefined, unref: () => undefined };
      },
    },
    18555,
  );
  expect(await server.ensure()).toBe(true);
  expect(spawned).toStrictEqual([]);
});

test("a missing binary fails at once and is not retried", async () => {
  const reports: string[] = [];
  const deps: ServerDeps = {
    exists: () => false,
    fetch: () => Promise.reject(Object.assign(new Error("refused"), { code: "ECONNREFUSED" })),
    home: "/Users/x",
    now: () => 0,
    platform: "darwin",
    report: (message) => {
      reports.push(message);
    },
    sleep: () => Promise.resolve(),
    spawn: () => ({ once: () => undefined, unref: () => undefined }),
  };
  const server = createServer(deps, 18555);
  expect(await server.ensure()).toBe(false);
  expect(await server.ensure()).toBe(false);
  expect(reports).toStrictEqual([SETUP_HINT]);
});

test("the server is polled until it answers", async () => {
  const calls = { count: 0 };
  const child: Child = {
    once: () => undefined,
    unref: () => undefined,
  };
  const server = createServer(
    {
      exists: () => true,
      fetch: () => {
        calls.count += 1;
        return calls.count === 1
          ? Promise.reject(Object.assign(new Error("refused"), { code: "ECONNREFUSED" }))
          : Promise.resolve(response(true, {}));
      },
      home: "/Users/x",
      now: () => 0,
      platform: "darwin",
      report: () => undefined,
      sleep: () => Promise.resolve(),
      spawn: () => child,
    },
    18555,
  );
  expect(await server.ensure()).toBe(true);
  expect(calls.count).toBe(2);
});

test("a start that never becomes ready is reported", async () => {
  const reports: string[] = [];
  const clock = { now: 0 };
  const server = createServer(
    {
      exists: () => true,
      fetch: () => Promise.reject(Object.assign(new Error("refused"), { code: "ECONNREFUSED" })),
      home: "/Users/x",
      now: () => clock.now,
      platform: "darwin",
      report: (message) => {
        reports.push(message);
      },
      sleep: () => {
        clock.now = 60_000;
        return Promise.resolve();
      },
      spawn: () => ({
        once: (_event, listener) => {
          listener(new Error("enoent"));
        },
        unref: () => undefined,
      }),
    },
    18555,
  );
  expect(await server.ensure()).toBe(false);
  expect(reports).toStrictEqual([
    "plamo-translate: starting the PLaMo server failed: enoent",
    SETUP_HINT,
  ]);
});

test("an unexpected health error propagates", async () => {
  const server = createServer(
    {
      exists: () => true,
      fetch: () => Promise.reject(new Error("dns")),
      home: "/Users/x",
      now: () => 0,
      platform: "darwin",
      report: () => undefined,
      sleep: () => Promise.resolve(),
      spawn: () => ({ once: () => undefined, unref: () => undefined }),
    },
    18555,
  );
  await expect(server.ensure()).rejects.toThrow("dns");
});

test("a completion returns the model text", async () => {
  const server = createServer(
    {
      exists: () => true,
      fetch: (_url, init) => {
        expect(init?.body).toBe(
          '{"prompt":"<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>Hi<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>ja<|plamo:key|>form<|plamo:val|>polite<|plamo:msg|>","max_tokens":72,"temperature":0,"stop":["<|plamo:tag|>","<|plamo:bos|>"]}',
        );
        return Promise.resolve(response(true, { choices: [{ text: "こんにちは" }] }));
      },
      home: "/Users/x",
      now: () => 0,
      platform: "darwin",
      report: () => undefined,
      sleep: () => Promise.resolve(),
      spawn: () => ({ once: () => undefined, unref: () => undefined }),
    },
    18555,
  );
  expect(await server.complete("Hi", "ja")).toBe("こんにちは");
});

test("a failed completion is reported to the caller", async () => {
  const server = createServer(
    {
      exists: () => true,
      fetch: () => Promise.resolve(response(false, {}, 500)),
      home: "/Users/x",
      now: () => 0,
      platform: "darwin",
      report: () => undefined,
      sleep: () => Promise.resolve(),
      spawn: () => ({ once: () => undefined, unref: () => undefined }),
    },
    18555,
  );
  await expect(server.complete("Hi", "en")).rejects.toThrow(
    "plamo-translate: completion failed: 500",
  );
});
