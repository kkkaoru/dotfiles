# Third-party notices

`pi-my-gemini-api` contains no third-party source code. It runs the official CLI
and translates between it and Pi:

- **gemini-api-cli** by Google — <https://github.com/google-gemini/gemini-api-cli>,
  installed separately and never vendored. Its `generate --body` /
  `models list` commands and its Interactions API request and response models
  (`gemini-api generate --schema`) define what this provider sends and reads.
- **pi-antigravity** by Rahul Arya (MIT) — the earlier Pi provider, kept in Git
  history as the reference for how a Pi provider maps a transcript onto Google's
  tool-call shapes. No code copied.
