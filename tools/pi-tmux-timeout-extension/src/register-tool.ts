// This TypeScript file is executed with Bun.
import type { TmuxExtensionHost } from "../index.ts";
import { TMUX_LAUNCH_TIMEOUT_MILLISECONDS, type TmuxLaunch, type TmuxRuntime } from "./tmux.ts";
import { tmuxExecSchema } from "./tool-schema.ts";

function resultText(launch: TmuxLaunch): string {
  return [
    "Started detached tmux command.",
    `tmux session: ${launch.sessionName}`,
    `log: ${launch.logPath}`,
    `exit status: ${launch.statusPath}`,
  ].join("\n");
}

export function registerTmuxTool(host: TmuxExtensionHost, runtime: TmuxRuntime): void {
  host.registerTool({
    description:
      "Start a potentially slow, blocking, externally waiting, or duration-uncertain shell command in a detached tmux session and return immediately. Output and exit status are written to files under the system temporary directory.",
    executionMode: "parallel",
    label: "Tmux Exec",
    name: "tmux_exec",
    parameters: tmuxExecSchema,
    promptGuidelines: [
      "Prefer tmux_exec whenever a shell command may block, has uncertain duration, or could take at least 30 seconds; when in doubt, detach it. Use it by default for tests, builds, deploys, containers, database or data processing, model training or evaluation, external-state waits, network transfers, and broad repository inspections.",
      "Use foreground bash only for bounded local commands confidently expected to finish within 30 seconds. Give intentionally foregrounded network or otherwise risky commands an explicit timeout below 30 seconds, and give tmux_exec a realistic estimatedDurationSeconds.",
      "After tmux_exec starts a command, return control promptly; pi-tmux-timeout-extension will start a named continuation when its exit-status file appears.",
    ],
    promptSnippet: "Run potentially blocking or duration-uncertain shell work without blocking pi",
    async execute(_toolCallId, params, signal) {
      const launch: TmuxLaunch = runtime.createLaunch(params.command, params.estimatedDurationSeconds);
      const options: Parameters<TmuxExtensionHost["exec"]>[2] =
        signal === undefined
          ? { timeout: TMUX_LAUNCH_TIMEOUT_MILLISECONDS }
          : { signal, timeout: TMUX_LAUNCH_TIMEOUT_MILLISECONDS };
      const result: Awaited<ReturnType<TmuxExtensionHost["exec"]>> = await host.exec(
        "sh",
        ["-lc", launch.command],
        options,
      );
      if (result.code !== 0) {
        throw new Error(
          result.stderr.trim() || result.stdout.trim() || "Failed to start tmux command",
        );
      }
      runtime.trackLaunch(launch);
      return { content: [{ text: resultText(launch), type: "text" }], details: launch };
    },
  });
}
