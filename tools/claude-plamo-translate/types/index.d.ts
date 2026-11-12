// AskUserQuestion dialogs seen this session: JSON of the tool's questions -> the translated questions.
export type Dialogs = Record<string, unknown[]>

declare module 'claude-code' {
  interface PluginState {
    'plamo-translate': { dialogs: Dialogs }
  }
}
