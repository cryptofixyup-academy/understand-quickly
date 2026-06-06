# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

A public, machine-readable registry of code-knowledge graphs. `registry.json` stores only pointers (URLs) to graph files that live in source repos. Nightly sync fetches each URL, schema-validates it, and updates entry status fields. AI agents fetch `registry.json`, filter by `status: "ok"`, then fetch `graph_url` directly — no auth, no SDK.

## Commands

### Root package (registry scripts + tests)

```bash
nvm use              # pin to Node 20 (see .nvmrc)
npm install
npm test             # node:test on scripts/__tests__/*.test.mjs
npm run test:coverage  # same + c8 gates (≥90% lines/functions, ≥80% branches)
node --test scripts/__tests__/validate.test.mjs  # run a single test file
node --test --test-name-pattern "duplicate id" scripts/__tests__/validate.test.mjs  # filter by name
npm run validate     # validate registry.json shape + fetch+validate each graph URL
npm run smoke        # dry-run sync against tests/registry-smoke.json (offline-friendly)
npm run sync         # resync all entries, writes registry.json
npm run render       # regenerate README table between <!-- BEGIN/END ENTRIES --> markers
npm run aggregate    # build site/stats.json cross-graph stats
npm run badges       # regenerate site/badges/*.svg
npm run well-known   # regenerate site/.well-known/repos.json
```

During CI, `CHANGED_IDS=owner/repo,owner2/repo2` scopes `npm run validate` to only the changed entries. Setting it empty (the default) validates all.

### MCP server (`mcp/`)

```bash
cd mcp
npm install
npm run dev      # run via tsx (no build needed)
npm run build    # compile TypeScript → dist/
npm run start    # run compiled dist/index.js
npm test         # tsx --test tests/registry.test.ts tests/tools.test.ts
```

### CLI (`cli/`)

```bash
cd cli
npm install
npm test         # node:test on tests/*.test.mjs
```

#### `add` command resolution pipeline (`cli/src/add.mjs`)

`understand-quickly add` runs three resolution steps before building the entry:

| Step | Function | Source priority |
|---|---|---|
| 1 | `resolveId(flags)` | `--id` flag → `git remote get-url origin` → error |
| 2 | `resolveGraph(flags, repoRoot)` | `--graph-url` flag → local file scan (`findGraphFiles`) → format sniff (`sniffGraphFile`) → interactive picker |
| 3 | `resolveDescription(flags, id)` | `--description` flag → interactive prompt → `"Knowledge graph for <id>."` fallback |

`resolveGraph` returns `{ format, sourcePath, sniffed }`. If no graph file exists and no `--graph-url` or `--format` is given, it throws. In non-TTY mode, interactive pickers are skipped and missing info causes hard errors.

## Architecture

### Monorepo layout

Three independently-packaged workspaces sharing a git repo but **not** a root `workspaces` field — each subdirectory has its own `package.json` and `node_modules`. Install separately per package.

```
registry.json          ← canonical data file; only file sync.mjs writes to
schemas/               ← JSON Schema draft-2020-12 per format (<name>@<int>.json)
schemas/__fixtures__/  ← ok.json + bad.json per format (required for tests)
scripts/               ← Node.js ESM scripts for registry management
site/                  ← static frontend + pre-generated assets (badges, .well-known)
tests/                 ← Playwright E2E tests
mcp/src/               ← TypeScript MCP server
cli/src/               ← Node.js ESM CLI
```

### Data flow

```
registry.json  →  sync.mjs    (fetches graph_url, validates, updates status fields)
             →  render-readme.mjs  (writes README table)
             →  aggregate.mjs     (writes site/stats.json)
             →  well-known.mjs    (writes site/.well-known/repos.json)
```

`registry.json` is the single write target for all sync operations. The `shard.mjs` module adds a **read-only** transparent merge of `entries/<a-z0-9>.json` shards — no write/migration logic exists yet; sharding is future work triggered at >1000 entries.

### Scripts (`scripts/`)

| File | Purpose |
|---|---|
| `validate.mjs` | Registry shape validation (via `meta.schema.json`) + per-entry graph fetch+validate. Exports `validateRegistry`, `validateGraph`, `fetchAndValidate`. |
| `sync.mjs` | Fetches every entry's `graph_url`, updates status/stats fields, writes `registry.json` atomically (tmp→rename). Exports `syncEntry`, `checkDrift`, `selectDriftBatch`. |
| `extract.mjs` | Per-format stats extraction (`extractStats`, `extractSourceSha`) and adversarial-graph structural caps (`validateBodyLimits`). |
| `shard.mjs` | `loadRegistry` merges `registry.json` + any `entries/*.json` shards. Used by all scripts that read the registry. |
| `render-readme.mjs` | Rewrites the `<!-- BEGIN ENTRIES --> ... <!-- END ENTRIES -->` block in README.md. |
| `aggregate.mjs` | Cross-graph aggregation → `site/stats.json`. Bounded parallel fetches (concurrency 6, 30s timeout per entry). |
| `issue-to-entry.mjs` | Parses GitHub issue body from `add-repo.yml` template → registry entry. Used by the `add-from-issue` workflow. |

### MCP server (`mcp/src/`)

Four tools registered in `index.ts`:
- `list_repos` — filter entries by format/tag/status
- `get_graph` — fetch a single graph by registry id
- `search_concepts` — search `stats.json` concepts or fall back to cross-graph node scan
- `find_graph_for_repo` — look up by `owner/repo` id or GitHub URL, fuzzy-match on miss

`registry.ts` owns all registry I/O: in-memory TTL cache (60s default), SSRF guard (blocks non-HTTPS, private IPv4/6, loopback, metadata endpoints), forward-compat `schema_version` check. Configurable via `UNDERSTAND_QUICKLY_REGISTRY` and `UNDERSTAND_QUICKLY_STATS` env vars.

Source resolution in `registry.ts`:
- `resolveRegistrySource()` — returns `UNDERSTAND_QUICKLY_REGISTRY` env var or `DEFAULT_REGISTRY_URL`
- `resolveStatsSource()` — returns `UNDERSTAND_QUICKLY_STATS` env var or `DEFAULT_STATS_URL`

Both are called at request time (not at startup), so the env vars can be changed between calls during tests.

### Graph formats

Each format is a versioned JSON Schema at `schemas/<name>@<int>.json`. The sync pipeline validates body shape against these schemas post-fetch. The `extract.mjs` extractors know each format's node/edge/language fields.

Format sniffing (in `cli/src/detect.mjs`) for auto-detecting local graph files:
- `understand-anything@1`: `metadata.tool === "understand-anything"`
- `gitnexus@1`: top-level `graph.nodes` + `graph.links`
- `code-review-graph@1`: `nodes` + `edges` + `stats.nodes_by_kind`
- `generic@1`: `nodes` + `edges` with no stats

## Key conventions

**Registry entries** must have `id` (`owner/repo` pattern), `owner`, `repo`, `format` (`^[a-z0-9-]+@[0-9]+$`), `graph_url` (HTTPS), `description` (≤200 chars). Entries are sorted by `id` in `registry.json` — `sync.mjs` re-sorts on every write.

**Adding a new format**: add `schemas/<name>@<int>.json` (JSON Schema draft-2020-12) + `schemas/__fixtures__/<name>/ok.json` + `bad.json`. Then add a row to the README formats table. `npm test` validates fixtures automatically.

**README table**: the `<!-- BEGIN ENTRIES -->` / `<!-- END ENTRIES -->` block is auto-generated by `render-readme.mjs`. Never hand-edit between those markers.

**Adversarial-graph caps** (in `extract.mjs`): 100k nodes, 500k edges, 4096-char labels, 32 levels of nesting. These run before schema validation so Ajv is never given a pathological body.

**Drift detection**: `sync.mjs` runs up to 25 drift checks per sync run via the unauthenticated GitHub API (60 req/hr budget), rotating through entries via `last_drift_index`. Drift failures never change entry `status` — they only set `head_sha`/`commits_behind`/`drift_checked_at`.

**Entry lifecycle status** values: `pending → ok | missing | invalid | oversize | transient_error` → `dead` (7 consecutive misses); `revoked` is maintainer-only and frozen (sync skips it entirely).
