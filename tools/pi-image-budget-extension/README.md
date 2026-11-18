# pi-image-budget-extension

Keeps one provider request under the model's image cap.

Pi's model catalog can describe hard limits with `inputLimits.images.maxPerRequest` and
`maxPerMessage`, but Pi does not rewrite history to respect them: a long session that has read
images sends all of them, and a provider such as Workers AI answers
`400 Invalid data for image - too many images in request (max 32)`.

This extension runs before each model call (`context`) and, only when the selected model declares
one of those counts, replaces images with
`[image omitted: only the newest images of this conversation are sent]`: the newest
`maxPerRequest` images of the request stay, and so do the newest `maxPerMessage` of each message.
The session itself is untouched — the transcript still shows every image — and one line says what
was left out the first time it happens.

The limit comes from the catalog, so declare it in `~/.pi/agent/models.json`:

```json
"cloudflare-ai-gateway": {
  "modelOverrides": {
    "workers-ai/@cf/deepseek-ai/deepseek-v4.1-flash-1017": {
      "inputLimits": { "images": { "maxPerMessage": 4, "maxPerRequest": 8 } }
    }
  }
}
```

A model without a declared cap is left alone. `/reload` or a new session picks up a change.

## Check

```sh
bun install
bun run check
```
