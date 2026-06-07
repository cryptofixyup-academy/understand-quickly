# API Reference

## registry.json Structure

The canonical registry file contains a JSON object with an `entries` array. Each entry describes a code-knowledge graph that can be queried by AI agents.

### Registry Object

```typescript
interface Registry {
  entries: Entry[];
}
```

### Entry Object

Each entry is a JSON object with these fields:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | ✓ | Unique identifier in format `owner/repo` (e.g., `"cryptofixyup-academy/understand-quickly"`) |
| `owner` | string | ✓ | Repository owner/organization name |
| `repo` | string | ✓ | Repository name (no slashes) |
| `format` | string | ✓ | Graph format version (e.g., `"understand-anything@1"`, `"gitnexus@1"`) |
| `graph_url` | string | ✓ | HTTPS URL to the knowledge graph JSON file |
| `description` | string | ✓ | Human-readable description (≤200 characters) |
| `status` | string | ✓ | Current entry status (see [Entry Status](#entry-status-lifecycle)) |
| `tags` | string[] | | Metadata tags (optional, e.g., `["python", "ml"]`) |
| `last_synced` | string (ISO 8601) | | Timestamp of last successful fetch/validation |
| `source_sha` | string | | Git SHA of the graph file's source (for drift tracking) |
| `head_sha` | string | | Latest commit SHA on repo's default branch (from GitHub API) |
| `commits_behind` | number | | Commits between `source_sha` and `head_sha` (drift indicator) |
| `drift_checked_at` | string (ISO 8601) | | When drift was last checked |
| `last_error` | string | | Last validation error message (when status is `invalid` or `missing`) |
| `miss_count` | number | | Consecutive fetch/validation failures (if ≥7, status → `dead`) |

### Entry Status Lifecycle

Entries transition through these statuses:

```
pending  ──→  ok       (graph validates)
         ├─→  missing  (404 on fetch)
         ├─→  invalid  (schema validation fails)
         ├─→  oversize (exceeds structural caps)
         └─→  transient_error  (temporary fetch failure)
              
              ↓ (7 consecutive misses)
              
              dead    (frozen; sync skips it)
```

**Special status:**
- `revoked` — maintainer-only, manually set; frozen during sync (no status transitions)

### Example Entry

```json
{
  "id": "nodejs/node",
  "owner": "nodejs",
  "repo": "node",
  "format": "understand-anything@1",
  "graph_url": "https://raw.githubusercontent.com/nodejs/node/main/.understand-anything/knowledge-graph.json",
  "description": "Node.js runtime written in C++, JavaScript bindings via native modules",
  "status": "ok",
  "tags": ["javascript", "runtime", "c++"],
  "last_synced": "2025-05-26T02:00:00Z",
  "source_sha": "abc123def456",
  "head_sha": "xyz789uvw012",
  "commits_behind": 5,
  "drift_checked_at": "2025-05-26T01:55:00Z"
}
```

## Fetching registry.json

The canonical registry is published at:

```
https://looptech-ai.github.io/understand-quickly/registry.json
```

### Usage Example (JavaScript/TypeScript)

```javascript
// Fetch and parse registry
const response = await fetch('https://looptech-ai.github.io/understand-quickly/registry.json');
const registry = await response.json();

// Filter to only "ok" entries
const readyEntries = registry.entries.filter(e => e.status === 'ok');

// Find a specific repo
const entry = readyEntries.find(e => e.id === 'nodejs/node');

// Fetch the knowledge graph
const graphResponse = await fetch(entry.graph_url);
const graph = await graphResponse.json();
```

### Usage Example (Python)

```python
import requests

# Fetch registry
registry = requests.get(
    'https://looptech-ai.github.io/understand-quickly/registry.json'
).json()

# Filter and find
ready = [e for e in registry['entries'] if e['status'] == 'ok']
entry = next((e for e in ready if e['id'] == 'nodejs/node'), None)

if entry:
    # Fetch the knowledge graph
    graph = requests.get(entry['graph_url']).json()
```

### Usage Example (curl)

```bash
# Get the registry
curl -fsSL https://looptech-ai.github.io/understand-quickly/registry.json \
  | jq '.entries[] | select(.status == "ok") | select(.id == "nodejs/node")'

# Or get all "ok" entries for a specific format
curl -fsSL https://looptech-ai.github.io/understand-quickly/registry.json \
  | jq '.entries[] | select(.status == "ok") | select(.format == "gitnexus@1")'
```

## Drift Detection

When an entry's graph file hasn't been updated since the repo's default branch advanced, we flag it as "drifted":

- **`commits_behind`** — number of commits between the graph file's source SHA and the repo's HEAD
- **`drift_checked_at`** — when this check last ran (once per nightly sync, rotating through entries)

Drift checking is rate-limited to ~25 entries per sync run (GitHub API budget: 60 req/hr unauthenticated).

**Important:** Drift never changes an entry's `status` field — it only updates these three fields. A drifted graph with `status: "ok"` is still queryable; the drift metadata helps consumers decide if they want fresher data.

## Stats.json (Cross-Graph Aggregation)

Nightly aggregation produces a smaller dataset at:

```
https://looptech-ai.github.io/understand-quickly/stats.json
```

Structure:

```typescript
interface Stats {
  generated_at: string; // ISO 8601 timestamp
  snapshot_commit: string; // registry.json commit SHA
  total_entries: number;
  entries_by_status: Record<string, number>; // { ok: N, invalid: M, ... }
  entries_by_format: Record<string, number>; // { "understand-anything@1": N, ... }
  concepts: Array<{ name: string; frequency: number; sample_repos: string[] }>;
  languages: Array<{ name: string; frequency: number }>;
}
```

Use `stats.json` for dashboards, publishing statistics, or quick concept lookups without fetching individual graphs.

## Well-Known Discovery

RFC 8615 endpoints for discovery without hitting the registry first:

```
/.well-known/code-graph.json      "About us" — registry URLs, service metadata
/.well-known/repos.json           Flat list of "ok" entries (agent-friendly)
/.well-known/code-graph-discovery.html  Browser UI for repo lookup
```

Hosted at: `https://looptech-ai.github.io/understand-quickly/.well-known/`

## Schema Versioning

Graph formats use versioning to handle evolution:

- Format: `{name}@{version}` (e.g., `"understand-anything@1"`)
- Breaking changes increment the version (e.g., → `@2`)
- Entries can specify different format versions
- Old versions continue to work; new generators should target latest

See [`docs/spec/code-graph-protocol.md`](spec/code-graph-protocol.md) for the full protocol specification.

## Caching & Freshness

- **Registry**: ~immutable; publish to GitHub Pages on each sync
- **Graph files**: Vary by producer; cache headers depend on their hosting
- **Stats**: Updated nightly (~2 UTC)
- **MCP server**: In-memory TTL cache (default 60 seconds); configurable via `UNDERSTAND_QUICKLY_REGISTRY` env var

## Rate Limits & Quotas

- **GitHub API**: 60 requests/hour (unauthenticated); drift checking rotates through entries
- **Registry file size**: No hard limit; sharding planned for >1000 entries
- **Graph validation**: Adversarial caps: 100k nodes, 500k edges, 4096-char labels, 32 nesting levels
- **Concurrent graph fetches**: Default 6 (aggregate.mjs); configurable per tool

## Error Codes & Meanings

When an entry has `status != "ok"`, inspect `last_error`:

| Status | Example Error | Likely Cause |
|--------|---------------|--------------|
| `missing` | `HTTP 404` | Graph file was deleted or URL changed |
| `invalid` | `Schema validation failed: nodes.0.id is required` | Graph doesn't match expected format |
| `oversize` | `Nodes exceeded 100k limit` | Graph file is too large |
| `transient_error` | `Timeout after 30s` | Temporary network issue; will retry next sync |
| `dead` | `7 consecutive failures` | Frozen; requires manual intervention |

See [Troubleshooting](troubleshooting.md) for recovery steps.
