// The host as a test sets it: plain data read through the same functions the mods import.
// `calls` records every action a mod asks the host for, in order. `reset()` restores defaults.
const defaults = () => ({
  agents: [],
  terminals: [],
  panes: [],
  spaces: [],
  machines: [],
  focused: null,
  theme: {},
  shared: {},
  props: null,
  window: {
    platform: "macos",
    titlebar: "none",
    buttons: false,
    maximized: false,
    fullscreen: false,
    active: true,
  },
  layout: null,
  settings: { zoom: 1 },
  exec: async () => ({ code: 0, stdout: "", stderr: "" }),
  hc: async () => ({ code: 0, stdout: "", stderr: "" }),
  run: async () => ({}),
  watchFolder: async () => [],
  nextCommand: async () => "",
  calls: [],
});

export const host = defaults();
export function reset() {
  for (const key of Object.keys(host)) {
    delete host[key];
  }
  Object.assign(host, defaults());
}

const record = (name, args, result) => {
  host.calls.push([name, ...args]);
  return result;
};

export const agents = () => host.agents;
export const terminals = () => host.terminals;
export const panes = () => host.panes;
export const spaces = () => host.spaces;
export const machines = () => host.machines;
export const focused = () => host.focused;
export const theme = () => host.theme;
export const shared = (key) => host.shared[key];
export const props = () => host.props;
export const window = () => host.window;
export const layout = () => host.layout;
export const settings = () => host.settings;
export function setShared(key, value) {
  host.shared[key] = value;
  return record("setShared", [key, value], null);
}
export const setAgentOrder = (addresses) => record("setAgentOrder", [addresses], null);
export const openOverlay = (spec) => record("openOverlay", [spec], "overlay-1");
export const closeOverlay = (id) => record("closeOverlay", [id], null);
export const run = (...args) => record("run", args, host.run(...args));
export const exec = (...args) => record("exec", args, host.exec(...args));
export const hc = (...args) => record("hc", args, host.hc(...args));
export const watchFolder = (...args) => record("watchFolder", args, host.watchFolder(...args));
export const nextCommand = () => record("nextCommand", [], host.nextCommand());
