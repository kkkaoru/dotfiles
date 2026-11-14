import { element } from "./gpui-kit.js";

export const h_flex = () => element("h_flex");
export const v_flex = () => element("v_flex");
// Records each item it was asked to scroll to.
export const ScrollHandle = {
  new: () => {
    const handle = { items: [] };
    handle.scroll_to_item = (i) => handle.items.push(i);
    return handle;
  },
};
