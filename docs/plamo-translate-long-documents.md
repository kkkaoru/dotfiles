# PLaMo translate: documents longer than the 2048-token window

`plamo-swift` serves a 2048-token sliding window. Both translate mods sent the whole text as one
completion and asked for up to 4096 answer tokens, so anything past roughly 1500 Japanese
characters left the answer no room and the translation degraded without an error.

## What the degradation looks like

A 3780-character Japanese document measures **1681 prompt tokens**. The old single request asked for
`1681 + 4096 = 5777` tokens of a 2048-token window:

| request | wanted | window | answer |
|---|---|---|---|
| single shot (old) | 1681 prompt + 4096 answer = 5777 | 2048 | 21 108 chars for a 3780-char source, ending mid-sentence |
| chunked (new) | 938 + 1020 = 1958 | 2048 | 5879 chars |
| chunked (new) | 744 + 812 = 1556 | 2048 | 4500 chars |

The single-shot answer is 5.6× the length of its source and stops inside a sentence
(`"long documents won't break.\nIf you split"`): the model kept decoding because the window rolled
its own prefill out, not because there was more to translate. The chunked run returns 10 379 chars
in total, two complete translations of their own chunks, in 5.7 s against the single request's
14.1 s.

A second identical run came back byte for byte with 0.0 s per request: `CachingProvider` serves the
repeat from the prompt cache, which is also how the byte-identity of the output was checked.

## What shipped

**`POST /v1/tokenize` on plamo-swift.** `{"texts": ["…", "…"]}` answers
`{"window": 2048, "tokens": [4, 3], "max_tokens": 4096}`. The counts are exactly what `complete`
prefills for the same text (`ModelRuntime.countTokens` is `encode(text).count`, BOS included), so a
client can size its chunks instead of guessing from characters. A server built without a tokenizer
answers `501` and the client keeps its old single request.

**Chunking in the mod (`tools/pi-plamo-translate/src/chunk.ts`).**

1. `units` splits on Japanese and English sentence terminators, keeps a closing quote with the
   sentence it closes, and keeps the trailing space, so the units concatenate back to the input
   byte for byte.
2. `plan` measures the prompt template and every unit, then gives half the window to the source and
   half to the answer; a unit that does not fit is cut proportionally and re-measured (at most
   eight rounds, far past the accuracy of a proportional cut).
3. `pack` groups consecutive units within that budget, and each chunk asks for its own token count
   plus 16 as the answer, capped at the server's `max_tokens`.
4. The answers rejoin with `""` for Japanese and `" "` for English, and a text of 512 characters or
   fewer keeps the single request it always had, unchanged and with no extra tokenize round trip.

## Reproducing

```bash
cd tools/plamo-swift && swift build -c release
cd ../.. && ./scripts/plamo-swift-service.sh reload
cd tools/pi-plamo-translate && bun run scripts/measure-document.ts 60
```

`measure-document.ts` builds the document from a repeated sentence, prints its prompt tokens and the
window, then sends the old single request and the planned chunks, with the prompt tokens and answer
reserve of each one.

## Not done

- **Morphological analyzers (MeCab/Sudachi).** They would cut Japanese at clause boundaries rather
  than sentence ones, but they are a new runtime dependency and a model download, and a sentence is
  already a unit the model translates well.
- **Parallel chunk requests.** `Router` decodes one request at a time on a serial queue, so sending
  chunks together would queue them anyway; the serial order also keeps the answers in document
  order without reassembly bookkeeping.
