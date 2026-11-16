# Third-party notices

`pi-my-antigravity` is an independent, unofficial integration. It contains no
third-party source code; the wire format was derived from these references:

- **Antigravity CLI (`agy`) 1.5.9** by Google — the installed official client. Its
  endpoints, User-Agent fingerprint, OAuth scopes, public OAuth application and
  model catalog define what this provider sends. Not redistributed here.
- **google-antigravity (antigravity-sdk-python)** by Google — Apache-2.0. Used to
  understand agent configuration and Vertex/API-key modes; no code copied.
- **pi-antigravity** by Rahul Arya (MIT) — the existing Pi provider used to compare
  behaviour and to identify the `429 RESOURCE_EXHAUSTED` interaction with
  `request.systemInstruction` (upstream issue #137). No code copied.
