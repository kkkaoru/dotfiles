import { describe, expect, it } from "vitest";

import { CliError, findCli, runCli } from "../src/cli.ts";

describe("findCli", () => {
  it("prefers GEMINI_API_CLI", () => {
    expect(findCli({ GEMINI_API_CLI: "/opt/cli" }, () => false)).toBe("/opt/cli");
  });

  it("finds the first match on PATH", () => {
    const found = findCli({ PATH: "/a:/b" }, (path) => path === "/b/gemini-api");
    expect(found).toBe("/b/gemini-api");
  });

  it("falls back to ~/.local/bin", () => {
    const found = findCli({ PATH: "" }, (path) => path.endsWith("/.local/bin/gemini-api"));
    expect(found).toContain("/.local/bin/gemini-api");
  });

  it("returns undefined when nothing exists", () => {
    expect(findCli({ PATH: "/a" }, () => false)).toBeUndefined();
  });

  it("skips empty PATH entries", () => {
    expect(findCli({ PATH: "::/a" }, () => false)).toBeUndefined();
  });
});

describe("runCli", () => {
  it("collects stdout", async () => {
    const result = await runCli("/bin/echo", ["hello"]);
    expect(result.stdout.trim()).toBe("hello");
  });

  it("rejects a non-zero exit with the captured detail", async () => {
    const failure = runCli("/bin/sh", ["-c", "echo boom >&2; exit 3"]);
    await expect(failure).rejects.toBeInstanceOf(CliError);
    await expect(failure).rejects.toThrow(/exited with 3: boom/u);
  });

  it("rejects when the binary is missing", async () => {
    await expect(runCli("/nonexistent/gemini-api", [])).rejects.toThrow();
  });

  it("keeps the exit code and detail", () => {
    const error = new CliError(2, "  bad  ");
    expect(error.code).toBe(2);
    expect(error.detail).toBe("  bad  ");
    expect(error.name).toBe("CliError");
  });
});

describe("findCli fallbacks", () => {
  it("ignores an empty GEMINI_API_CLI", () => {
    expect(findCli({ GEMINI_API_CLI: "", PATH: "/a" }, (path) => path === "/a/gemini-api")).toBe(
      "/a/gemini-api",
    );
  });
});

describe("runCli environment", () => {
  it("passes an explicit environment to the child", async () => {
    const result = await runCli("/bin/sh", ["-c", 'printf %s "$MARK"'], {
      env: { MARK: "yes" },
    });
    expect(result.stdout).toBe("yes");
  });

  it("reports exit code 1 when the child dies on a signal", async () => {
    await expect(runCli("/bin/sh", ["-c", "kill -9 $$"])).rejects.toMatchObject({ code: 1 });
  });
});

describe("findCli without PATH", () => {
  it("treats a missing PATH as empty", () => {
    expect(findCli({}, () => false)).toBeUndefined();
  });
});

describe("runCli abort", () => {
  it("rejects when the signal is already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(runCli("/bin/sleep", ["1"], { signal: controller.signal })).rejects.toThrow();
  });
});
