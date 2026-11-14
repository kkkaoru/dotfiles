import { afterEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import SystemStats, {
  parseBattery,
  parseMemoryUsed,
  parseCpuPercent,
  statsText,
} from "../../.config/herdr-app/mods/system-stats/main.js";

const IOSTAT = [
  "      cpu    load average",
  " us sy id   1m   5m   15m",
  " 32  6 62  4.84 6.62 9.12",
  " 20  3 77  4.84 6.62 9.12",
  "",
].join("\n");
const VM_STAT = [
  "Mach Virtual Memory Statistics: (page size of 16384 bytes)",
  "Pages free:                                     5694.",
  "Pages wired down:                             344677.",
  "Pages purgeable:                                7577.",
  "Anonymous pages:                             1962010.",
  "Pages occupied by compressor:                  32153.",
].join("\n");
const IOREG_CHARGING = [
  '      "CurrentCapacity" = 8',
  '      "Amperage" = 37',
  '      "ExternalConnected" = Yes',
  '      "PowerTelemetryData" = {"SystemEnergyConsumed"=23895,"SystemLoad"=85611,"SystemCurrentIn"=4474}',
  '      "MaxCapacity" = 100',
  '      "Voltage" = 11176',
].join("\n");
const IOREG_INTEL = [
  '      "CurrentCapacity" = 4000',
  '      "Amperage" = -1500',
  '      "ExternalConnected" = No',
  '      "MaxCapacity" = 5000',
  '      "Voltage" = 12000',
].join("\n");

// Replies to exec by command name; a cx whose spawned tasks and timers a test runs by hand.
function newStats(replies) {
  host.exec = async (command) => replies[command];
  host.theme = { dim: "#d", foreground: "#f", chrome: "#c", border: "#b" };
  const tasks = [];
  const timers = [];
  const cx = {
    notify: vi.fn(),
    spawn: (fn) => tasks.push(fn(cx)),
    timer: { every: (ms, fn) => timers.push([ms, fn]) },
  };
  const view = new SystemStats();
  view.init({}, cx);
  return { view, cx, tasks, timers };
}

const ALL = {
  ioreg: { code: 0, stdout: IOREG_CHARGING, stderr: "" },
  iostat: { code: 0, stdout: IOSTAT, stderr: "" },
  sysctl: { code: 0, stdout: "51539607552\n", stderr: "" },
  vm_stat: { code: 0, stdout: VM_STAT, stderr: "" },
  osascript: { code: 0, stdout: "281483714049\n", stderr: "" },
};

afterEach(() => {
  reset();
});

test("parseCpuPercent reads the busy share of the last sample, not the average since boot", () => {
  expect(parseCpuPercent(IOSTAT)).toBe(23);
});

test("parseMemoryUsed counts app memory, wired and compressed pages as Activity Monitor does", () => {
  expect(parseMemoryUsed(VM_STAT)).toBe(38195412992);
});

test("parseBattery takes the whole Mac's draw from SystemLoad when it is reported", () => {
  expect(parseBattery(IOREG_CHARGING)).toStrictEqual({ percent: 8, watts: 85.611 });
});

test("parseBattery falls back to the battery's voltage times current", () => {
  expect(parseBattery(IOREG_INTEL)).toStrictEqual({ percent: 80, watts: 18 });
});

test("parseBattery is null on a Mac without a battery", () => {
  expect(parseBattery("")).toBe(null);
});

test("statsText shows dashes before the first readings and no battery without one", () => {
  expect(statsText({ cpu: null, memory: null, free: null, battery: null })).toStrictEqual({
    cpu: "—",
    memory: "—",
    disk: "—",
    battery: null,
  });
});

test("statsText formats each reading", () => {
  const memory = { used: 38195412992, total: 51539607552 };
  const battery = { percent: 8, watts: 85.611 };
  expect(statsText({ cpu: 23, memory, free: 281483714049, battery })).toStrictEqual({
    cpu: "23%",
    memory: "35.6/48 GB",
    disk: "281 GB free",
    battery: "8% · 85.6 W",
  });
  expect(statsText({ cpu: 0, memory: null, free: null, battery: null }).cpu).toBe("0%");
});

test("init reads everything once and polls the battery, the load and the disk at their own pace", async () => {
  const { view, cx, tasks, timers } = newStats(ALL);
  await Promise.all(tasks);
  expect(timers.map(([ms]) => ms)).toStrictEqual([2000, 5000, 60000]);
  expect(cx.notify).toHaveBeenCalledTimes(3);
  expect(statsText(view)).toStrictEqual({
    cpu: "23%",
    memory: "35.6/48 GB",
    disk: "281 GB free",
    battery: "8% · 85.6 W",
  });
  timers.forEach(([, fn]) => fn(cx));
  await Promise.all(tasks);
  expect(cx.notify).toHaveBeenCalledTimes(6);
  expect(host.calls.map(([, command]) => command)).toStrictEqual([
    "ioreg",
    "iostat",
    "sysctl",
    "vm_stat",
    "osascript",
    "ioreg",
    "iostat",
    "sysctl",
    "vm_stat",
    "osascript",
  ]);
});

test("a failed command rejects its task with the command's error and keeps the last reading", async () => {
  const { view, tasks } = newStats({
    ...ALL,
    osascript: { code: 1, stdout: "", stderr: "boom\n" },
  });
  await expect(Promise.all(tasks)).rejects.toThrow("system-stats: osascript failed (1): boom");
  expect(view.free).toBe(null);
});

test("render shows the readings in the status bar, the battery only when there is one", async () => {
  const { view, tasks } = newStats(ALL);
  await Promise.all(tasks);
  const row = view.render({});
  expect(row.calls).toStrictEqual([
    ["size_full"],
    ["px", 14],
    ["gap", 14],
    ["items_center"],
    ["justify_end"],
    ["bg", "#c"],
    ["border_t", 1],
    ["border_color", "#b"],
    ["text_size", 11.8],
    ["when", true],
  ]);
  expect(row.kids.map((reading) => reading.kids.map((part) => part.kids[0]))).toStrictEqual([
    ["🧠", "23%"],
    ["🐏", "35.6/48 GB"],
    ["💾", "281 GB free"],
    ["🔋", "8% · 85.6 W"],
  ]);
  view.battery = null;
  expect(view.render({}).kids.length).toBe(3);
});
