# Replayed model-server streams

Each `<name>.sse` is a response body exactly as llama-server sends it (OpenAI-compatible SSE, or a JSON
error body), and `<name>.expect.json` is what the client must make of it. The test
`llamaserver::replay_fixture_tests` serves every fixture in 1-byte, 7-byte and whole-body chunks and
checks the result, so no model is needed.

`expect.json` fields:

- `status` (default 200) and `content_type` (default `text/event-stream`): how the body is served.
- `text`, `finish_reason`, `reasoning_present`, `prompt_tokens`: the expected stream outcome.
- `error`: the expected failure class instead: `context_exceeded`, `unavailable`, `timeout`,
  `truncated`, `bad_request` or `server` (plus `prompt_tokens` / `context` for overflows).

To add a real failure: take `request`/`output` from a conversation export's `model_requests`, or
capture the raw body, save it here, and write the expectation. Every reported streaming, parsing or
saving bug should add one.
