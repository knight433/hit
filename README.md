# hitpoint

A terminal API tester for FastAPI backends — for you **and** your AI agents.
No more hand-crafted curl: register a project once, browse its endpoints by
tag, fill a schema-generated request form, and hit it. The same engine is
available headlessly as CLI subcommands with JSON output and as an **MCP
server**, so coding agents can discover and call your APIs natively.

```
binary: hit          (cargo build produces `hitpoint`; alias or rename as you like)
```

## How it works

When you add a project you give its base URL. hitpoint fetches
`{base_url}/openapi.json` from the running server (with an optional on-disk
spec file as fallback, plus a TTL cache), normalizes OpenAPI 3.0/3.1 into one
schema model, and generates request templates from it. FastAPI-specific
conventions (the `anyOf: [T, null]` Optional encoding, the
`OAuth2PasswordRequestForm` login shape, 422 `detail` rendering) are isolated
behind a framework adapter so other frameworks can be added later.

## Quick start

```sh
cargo install --path .          # or: cargo build --release

hit projects add billing --base-url http://localhost:8000
hit                             # open the TUI
```

### TUI

`projects → tags → endpoints → request form → response`

The form is generated from the endpoint's JSON schema: required fields are
marked `*`, path/query/header params get their own sections, enums cycle with
←/→, booleans toggle with space, arrays grow/shrink with `a`/`d`.

| Key | Action |
|---|---|
| `l` / `L` | log in / log out of the selected project (projects screen) — credentials are asked in a modal |
| `i` | toggle endpoint docs on the form (docstring description + expected response bodies); the endpoint list always shows docs for the highlighted endpoint |
| `enter` | edit field / toggle / expand |
| **`Shift+X`** | cycle a field: value → `∅ null` → `— excluded` (as allowed by required/nullable) |
| `x` | re-include an excluded/nulled field |
| `a` / `d` | append / delete array item |
| `e` | open the whole body in `$EDITOR` (escape hatch for anything the form can't express) |
| `ctrl+s` | send the request |
| `/` | filter endpoint list |
| `esc` | back |

`Shift+X` distinguishes **omitting a key** from **sending `null`** — they are
different things to FastAPI (especially PATCH endpoints with `exclude_unset`):

- optional + nullable field: value → null → excluded → value …
- required + nullable: value ↔ null (can't be omitted)
- optional, not nullable: value ↔ excluded (can't be null)

### Headless CLI (for scripts and agents)

Every command supports `--json` (automatic when stdout is not a TTY) and
prints a `{ok, data, error: {kind, message}}` envelope.

```sh
hit tags billing
hit endpoints billing --tag users --search create
hit template billing create_user_users__post     # fill-in-the-blanks template
hit run billing "POST /users/" --body '{"email": "a@b.c", "name": "Neo"}' \
    -q limit=10 -H 'X-Debug: 1'
hit run billing get_user_users__user_id__get -p user_id=u-42
hit login billing && hit logout billing
hit spec refresh billing
hit config check
hit completions zsh
```

Exit codes: `0` ok · `1` usage/config · `2` spec · `3` auth · `4` network ·
`5` HTTP 4xx · `6` HTTP 5xx (`--allow-error` forces 0 on HTTP errors).

### MCP server (for AI agents)

```sh
claude mcp add hitpoint -- hit mcp        # Claude Code
```

Tools: `list_projects`, `list_tags`, `list_endpoints`,
`get_request_template`, `execute_request`. Templates include the example
body, the full body schema, and `optional_paths` / `nullable_paths` so the
agent knows what it may omit and what accepts null. MCP mode never opens a
browser or prompts — interactive auth fails with the exact `hit login …`
command to run instead.

## Configuration

`~/.config/hitpoint/projects.toml` (or `--config <file>`):

```toml
[settings]
spec_cache_ttl_secs = 300   # re-fetch openapi.json after this
timeout_secs = 30
token_store = "auto"        # auto | keyring | file

[projects.billing]
base_url = "http://localhost:8000"
spec_file = "/path/to/openapi.json"          # optional offline fallback
default_headers = { "X-Tenant" = "dev" }

# Local auth: POST to a login endpoint, cache the JWT, re-login before exp.
[projects.billing.auth]
type = "jwt_login"
login_path = "/auth/login"
login_content_type = "form"                  # form = OAuth2PasswordRequestForm
username = { env = "BILLING_USER" }          # or { value = "..." } | { keyring = "entry" } | { prompt = true }
password = { env = "BILLING_PASS" }
token_json_pointer = "/access_token"
refresh_margin_secs = 60

# 3rd-party auth: browser-based OAuth2 authorization-code + PKCE.
[projects.crm]
base_url = "https://crm.internal.example.com"

[projects.crm.auth]
type = "oauth2_pkce"
auth_url = "https://idp.example.com/authorize"
token_url = "https://idp.example.com/oauth/token"
client_id = "hitpoint-cli"
scopes = ["openid", "api:read"]
redirect_port = 0            # 0 = ephemeral; set fixed if your IdP requires it
```

Auth is attached as `Authorization: Bearer …`; a 401 invalidates the cached
token and retries once (re-login or refresh-token grant). Logging in works
everywhere it can: `hit login <project>` prompts on the terminal, the TUI
prompts in a modal (press `l` on the projects screen, or just send a request
— you'll be asked when credentials are needed), and OAuth opens the browser
from either. Only MCP mode never prompts; it returns the `hit login …`
command to run instead. Tokens are stored
in the OS keyring when built with `--features keyring` (with automatic
fallback to `0600` files under `~/.local/share/hitpoint/tokens/`), or in
files otherwise. `HITPOINT_NO_BROWSER=1` prints the OAuth URL instead of
launching a browser (SSH sessions).

## Development

```sh
cargo test                          # unit + integration (wiremock, insta, TUI TestBackend)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Layout: `spec/` (fetch, $ref resolution, 3.0/3.1 normalization — the critical
module), `model/` (SchemaNode/Endpoint/RequestTemplate shared by every
frontend), `http/` (executor + 401 retry), `auth/` (provider trait: jwt_login,
oauth2_pkce), `cli/`, `mcp/`, `tui/` (screen stack + form engine). Logs:
CLI → stderr (`-v`/`-vv`), TUI/MCP → `~/.local/share/hitpoint/logs/`.
