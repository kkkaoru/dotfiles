// The Mac's CPU use, memory in use, free disk space, battery charge and power draw, refreshed
// while the block is in the layout. Everything comes from commands macOS ships with; nothing to build or install.

import { View, div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, theme } from "herdr";

// One ioreg read of the battery node takes about 20 ms, cheap enough to follow the draw live.
const BATTERY_MS = 2000;
const LOAD_MS = 5000;
// osascript takes a quarter of a second to start, and free space moves slowly.
const DISK_MS = 60000;
const GIB = 1024 ** 3;
const GB = 1e9;
const MILLI = 1000;
// Finder's free space counts purgeable files macOS deletes on demand; `df` leaves them out and
// reports far less on an APFS startup disk. Foundation knows it, reached through JXA.
const DISK_SCRIPT = [
  'ObjC.import("Foundation");',
  "const free = $.NSURLVolumeAvailableCapacityForImportantUsageKey;",
  'const values = $.NSURL.fileURLWithPath("/").resourceValuesForKeysError($([free]), null);',
  "values.objectForKey(free).js;",
].join(" ");

const run = async (command, args) => {
  const { code, stdout, stderr } = await exec(command, args);
  if (code !== 0) {
    throw new Error(`system-stats: ${command} failed (${code}): ${stderr.trim()}`);
  }
  return stdout;
};

const vmCount = (vmStat, label) => Number(vmStat.match(new RegExp(`${label}:\\s+(\\d+)`))?.[1]);

/** `iostat -n 0 -c 2 -w 1` -> percent of CPU busy over its last one-second sample (the first
 *  row averages since boot). */
export function parseCpuPercent(iostat) {
  const idle = Number(iostat.trim().split("\n").at(-1).trim().split(/\s+/)[2]);
  return 100 - idle;
}

/** Bytes in use as Activity Monitor counts them: app memory, wired and compressed. */
export function parseMemoryUsed(vmStat) {
  const pageSize = Number(vmStat.match(/page size of (\d+) bytes/)[1]);
  const pages =
    vmCount(vmStat, "Anonymous pages") -
    vmCount(vmStat, "Pages purgeable") +
    vmCount(vmStat, "Pages wired down") +
    vmCount(vmStat, "Pages occupied by compressor");
  return pages * pageSize;
}

const ioregNumber = (ioreg, key) => Number(ioreg.match(new RegExp(`"${key}" ?= ?(-?\\d+)`))?.[1]);

/** `ioreg -rn AppleSmartBattery -d 1` -> charge in percent and watts drawn; null
 *  on a Mac without a battery. The draw is the whole Mac's (SystemLoad, Apple silicon), else the
 *  battery's own voltage times current. */
export function parseBattery(ioreg) {
  if (!ioreg.includes('"CurrentCapacity"')) {
    return null;
  }
  const systemLoad = ioregNumber(ioreg, "SystemLoad");
  const batteryWatts =
    (ioregNumber(ioreg, "Voltage") * Math.abs(ioregNumber(ioreg, "Amperage"))) / MILLI ** 2;
  return {
    percent: Math.round(
      (ioregNumber(ioreg, "CurrentCapacity") / ioregNumber(ioreg, "MaxCapacity")) * 100,
    ),
    watts: Number.isNaN(systemLoad) ? batteryWatts : systemLoad / MILLI,
  };
}

const oneDecimal = (value) => value.toFixed(1);

/** The three readings as shown; a reading not taken yet is a dash. */
export function statsText({ cpu, memory, free, battery }) {
  return {
    cpu: cpu === null ? "—" : `${cpu}%`,
    memory: memory ? `${oneDecimal(memory.used / GIB)}/${Math.round(memory.total / GIB)} GB` : "—",
    disk: free === null ? "—" : `${Math.round(free / GB)} GB free`,
    battery: battery && `${battery.percent}% · ${oneDecimal(battery.watts)} W`,
  };
}

export default class SystemStats extends View {
  init(_props, cx) {
    this.cpu = null;
    this.memory = null;
    this.free = null;
    this.battery = null;
    this.refreshBattery(cx);
    this.refreshLoad(cx);
    this.refreshDisk(cx);
    cx.timer.every(BATTERY_MS, (cx) => this.refreshBattery(cx));
    cx.timer.every(LOAD_MS, (cx) => this.refreshLoad(cx));
    cx.timer.every(DISK_MS, (cx) => this.refreshDisk(cx));
  }

  // A failed read is reported by cx.spawn and leaves the last reading shown.
  refreshBattery(cx) {
    cx.spawn(async (cx) => {
      this.battery = parseBattery(await run("ioreg", ["-rn", "AppleSmartBattery", "-d", "1"]));
      cx.notify();
    });
  }

  refreshLoad(cx) {
    cx.spawn(async (cx) => {
      const [iostat, memsize, vmStat] = await Promise.all([
        run("iostat", ["-n", "0", "-c", "2", "-w", "1"]),
        run("sysctl", ["-n", "hw.memsize"]),
        run("vm_stat", []),
      ]);
      this.cpu = parseCpuPercent(iostat);
      this.memory = { used: parseMemoryUsed(vmStat), total: Number(memsize) };
      cx.notify();
    });
  }

  refreshDisk(cx) {
    cx.spawn(async (cx) => {
      this.free = Number(await run("osascript", ["-l", "JavaScript", "-e", DISK_SCRIPT]));
      cx.notify();
    });
  }

  render(_cx) {
    const t = theme();
    const text = statsText(this);
    const reading = (label, value) =>
      h_flex()
        .gap(5)
        .child(div().text_color(t.dim).child(label))
        .child(div().text_color(t.foreground).child(value));
    return h_flex()
      .size_full()
      .px(14)
      .gap(14)
      .items_center()
      .justify_end()
      .bg(t.chrome)
      .border_t(1)
      .border_color(t.border)
      .text_size(11.8)
      .child(reading("🧠", text.cpu))
      .child(reading("🐏", text.memory))
      .child(reading("💾", text.disk))
      .when(Boolean(text.battery), (row) => row.child(reading("🔋", text.battery)));
  }
}
