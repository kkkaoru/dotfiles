// Elements that record their builder calls, so a test reads what a render asked for: children in
// `kids`, every other call in `calls` (callbacks included, for a test to invoke). `when(cond, fn)`
// applies `fn` as the real builder does.
export function element(kind, path) {
  const target = { kind, path, calls: [], kids: [] };
  const proxy = new Proxy(target, {
    get(t, key) {
      if (key in t && key !== "children") {
        return t[key];
      }
      if (key === "then") {
        return undefined;
      }
      return (...args) => {
        if (key === "child") {
          t.kids.push(args[0]);
        } else if (key === "children") {
          t.kids.push(...args[0]);
        } else if (key === "when") {
          t.calls.push(["when", Boolean(args[0])]);
          return args[0] ? args[1](proxy) : proxy;
        } else {
          t.calls.push([key, ...args]);
        }
        return proxy;
      };
    },
  });
  return proxy;
}

// What a view's `cx` and `this` need: a host test drives `init` and `render` by hand.
export class View {
  set_props(p) {
    this.props = p;
    this.update?.(p);
  }

  release() {
    this.released = true;
  }
}
export const div = () => element("div");
export const image = (path) => element("image", path);
export const svg = (path) => element("svg", path);
