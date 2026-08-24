# Configuration

mini-codex resolves provider settings in this order:

1. process environment;
2. `.env` in the startup workspace;
3. a built-in default, where one exists.

| Variable | Required | Meaning |
| --- | --- | --- |
| `OPENAI_API_KEY` | yes | Bearer credential for the Responses endpoint |
| `OPENAI_MODEL` | yes | Provider model identifier |
| `OPENAI_BASE_URL` | no | API root; defaults to `https://api.openai.com/v1` |

The adapter appends `/responses` to `OPENAI_BASE_URL`. DeepSeek's Responses API
therefore uses:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

Run `mini-codex status` to inspect the effective non-secret configuration and
its source. `status` never prints the credential. Run `mini-codex doctor` to
validate provider configuration and the host shell without starting an agent
turn. Both commands accept `--json`.

If the startup workspace contains `AGENTS.md`, mini-codex appends its UTF-8
contents once to the stable system prompt. The file has a 16 KiB hard limit;
startup fails explicitly rather than silently omitting or truncating oversized
instructions. Nested instruction discovery is not part of the v0.1 contract.
