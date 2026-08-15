import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

export default function registerProbe(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "cursor_bridge_probe",
    label: "Cursor Bridge Probe",
    description: "Return the fixed Cursor bridge TUI verification marker",
    parameters: Type.Object({}),
    async execute() {
      return {
        content: [{ type: "text", text: "CURSOR_BRIDGE_TOOL_OK" }],
        details: {},
      };
    },
  });
}
