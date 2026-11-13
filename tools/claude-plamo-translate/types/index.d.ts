// AskUserQuestion dialog texts seen this session: a question, header, label or description -> its translation.
export type Dialogs = Record<string, string>

declare module 'claude-code' {
  interface PluginState {
    'plamo-translate': { dialogs: Dialogs }
  }
}
