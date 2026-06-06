# Troubleshooting Guide

## My entry won't validate

### Problem: `status: "invalid"` with schema error

**Example error:** `Schema validation failed: nodes must be an array`

**Steps to fix:**
1. Download your graph file and inspect it locally
2. Check that it matches your declared `format` (see [Supported Formats](../README.md#supported-formats))
3. Validate against the schema:
   ```bash
   # Compare your graph against the schema
   curl -fsSL https://raw.githubusercontent.com/cryptofixyup-academy/understand-quickly/main/schemas/understand-anything@1.json \
     | jq . > schema.json
   
   # Use a JSON Schema validator (e.g., ajv-cli)
   npx ajv validate -s schema.json -d your-graph.json
   ```
4. If the error mentions a specific field, add/fix it:
   - Missing `nodes` or `edges`? Ensure the format produces both.
   - Invalid node/edge structure? Check field names (case-sensitive)
   - Nesting too deep? Flatten to ≤32 levels.

5. **Re-test:** Update your graph file in git, commit, and wait for the next nightly sync (~2 UTC).

### Problem: `status: "oversize"`

**Cause:** Your graph exceeds one of these structural caps:
- **Nodes:** >100,000
- **Edges:** >500,000
- **Label length:** >4,096 characters
- **Nesting depth:** >32 levels

**Solution:**
- **Too many nodes?** Filter by most-important symbols; omit low-value edges.
- **Labels too long?** Truncate verbose descriptions; use short IDs.
- **Deep nesting?** Flatten deep call stacks or inheritance hierarchies.

### Problem: Different tool, same format?

If you're using a different tool that outputs the same format (e.g., a custom tool that outputs `understand-anything@1`), make sure:
1. Your output structure exactly matches the schema (node fields, edge fields, metadata)
2. If there are optional fields you don't use, that's fine — omit them
3. Test your file locally first (see validation step above)

---

## My entry shows "drift"

### What is drift?

Your graph file hasn't been updated since new commits landed on the repo's default branch. Drift doesn't prevent querying — it just signals that the graph is stale.

**Fields that appear:**
- `commits_behind: 5` — 5 new commits since graph was generated
- `drift_checked_at: 2025-05-26T01:55:00Z` — last checked at this time

### How to fix

1. **Update your graph file** by re-running the generator:
   ```bash
   # Example: if you use understand-anything
   npx @understand-anything/generate > .understand-anything/knowledge-graph.json
   git add .understand-anything/knowledge-graph.json
   git commit -m "chore: regenerate knowledge graph"
   git push
   ```

2. **Wait for next nightly sync** (~2 UTC next day)

3. **Or trigger an instant refresh:**
   - Edit `registry.json` in a PR (even a trivial change like a whitespace edit)
   - Merge the PR
   - Sync runs immediately on merge to main

---

## Adding my repo failed

### Problem: "duplicate id"

**Cause:** An entry with your `owner/repo` already exists.

**Solutions:**
- Check [registry.json](../registry.json) for your repo
- If it's outdated/broken, [file an issue](https://github.com/cryptofixyup-academy/understand-quickly/issues) and request removal
- If it's yours, use the same entry; you can update it via PR

### Problem: "graph_url is unreachable"

**Cause:** The provided URL returns 404 or is not HTTPS.

**Steps to fix:**
1. Verify the URL is correct and public:
   ```bash
   curl -fsSLI https://your-url/graph.json
   ```
2. Ensure it's **HTTPS** (not HTTP)
3. Ensure it returns **200 OK** (not 302, 404, etc.)
4. Ensure the file is **publicly readable** (no auth required)
5. **Resubmit** via the [wizard](https://looptech-ai.github.io/understand-quickly/add.html) or PR

### Problem: "format not found"

**Cause:** The `format` field doesn't match a supported version.

**Solutions:**
1. Check supported formats: [`README.md#supported-formats`](../README.md#supported-formats)
2. Use one of:
   - `understand-anything@1`
   - `gitnexus@1`
   - `code-review-graph@1`
   - `bundle@1`
   - `generic@1`
3. If your tool outputs a different format, it may need a new schema; [file an issue](https://github.com/cryptofixyup-academy/understand-quickly/issues) to request support

### Problem: "description is too long"

**Cause:** Description exceeds 200 characters.

**Solution:** Truncate to ≤200 chars. Keep the key benefit or purpose in the first sentence.

---

## Adding my repo succeeded, but...

### I want to update my entry (graph_url, description, tags, etc.)

1. **Via PR:** Edit `registry.json`, modify the entry, and open a PR
2. **Via CLI:** `npx @understand-quickly/cli add --id=your/repo` (overwrites fields)
3. **Via wizard:** Fill the form again at https://looptech-ai.github.io/understand-quickly/add.html

Changes take effect after merge → next nightly sync.

### I want to remove my entry

1. **Via PR:** Delete the entry from `registry.json`
2. **Via issue:** Comment on your PR or [file an issue](https://github.com/cryptofixyup-academy/understand-quickly/issues) requesting removal

Removal is immediate after merge (no nightly sync delay).

### My entry is marked "dead"

**Cause:** 7 consecutive failed syncs (404, invalid schema, etc.).

**Solution:**
1. Fix the underlying issue (see [My entry won't validate](#problem-status-invalid-with-schema-error))
2. Open a PR to edit your entry (any change triggers an instant re-validation)
3. Merge the PR
4. The entry will be re-synced immediately

---

## Using the registry as an AI agent

### Problem: Getting 403 / "Forbidden" on graph fetch

**Cause:** The graph URL is not publicly accessible or requires authentication.

**Solution:** Ask the repo maintainer to ensure the graph file is:
- Publicly readable (no authentication)
- Hosted on a public URL (not behind a login wall)
- Served with CORS headers if accessed from a browser

### Problem: Graph structure is unexpected

**Cause:** Different generators produce different node/edge fields.

**Solution:** Inspect the graph and the format schema side-by-side:
1. Fetch the graph: `curl -fsSL <graph_url> | jq . | head -100`
2. Fetch the schema: `curl -fsSL https://raw.githubusercontent.com/cryptofixyup-academy/understand-quickly/main/schemas/<format>.json | jq .`
3. Adapt your code to handle the actual structure (e.g., `nodes[0].id` vs `nodes[0].identifier`)

### Problem: Search is returning no results

**Cause:** Using the MCP `search_concepts` tool, but the concept doesn't exist in any "ok" entry.

**Solution:**
1. Verify the concept name (substring match is case-sensitive by default)
2. Try a broader search (e.g., search for "parse" instead of "parseExpression")
3. Check which repos are in "ok" status: use `list_repos` tool or browse [registry.json](../registry.json)

---

## Performance & Limits

### Registry is slow to load

**Cause:** Registry file is large (entries > 1000) and/or slow network.

**Solution:**
- Cache locally (registry updates once per day)
- Use `.well-known/repos.json` instead (smaller, filtered to "ok" only)
- For stats/concepts, use `stats.json` (pre-aggregated)

### Graph fetch is timing out

**Cause:** Graph file is large (>500k edges) or hosted on slow server.

**Solution:**
1. Check file size: `curl -fsSLI <graph_url> | grep content-length`
2. If very large (>50 MB), consider splitting or sampling
3. Ask the host to optimize serving (CDN, compression)

### MCP search is slow / incomplete

**Cause:** Falling back to cross-graph fanout (limited to 5 entries, sequential).

**Solution:**
- Use `stats.json` for pre-computed concepts (faster, instant)
- Narrow search by format/tag to reduce fanout entries
- Pre-compute your own concept index for your use case

---

## CLI Troubleshooting

### `npx @understand-quickly/cli add` failed

#### Error: "Cannot find repo root"
**Cause:** Current directory is not a git repo or .git is missing.

**Fix:** Run from repo root: `cd /path/to/your/repo && npx @understand-quickly/cli add`

#### Error: "Multiple graph candidates found"
**Cause:** Multiple graph files match the expected locations.

**Fix:** Specify which one explicitly: `npx @understand-quickly/cli add --graph-url https://...`

#### Error: "gh command not found"
**Cause:** GitHub CLI (`gh`) is not installed.

**Fix:**
1. Install: https://github.com/cli/cli#installation
2. Authenticate: `gh auth login`
3. Retry

#### Error: "Cannot determine repo owner"
**Cause:** Git remote is SSH and couldn't be parsed.

**Fix:** Try HTTPS remote instead: `git remote set-url origin https://github.com/owner/repo.git`

---

## Still Stuck?

- **Check existing issues:** https://github.com/cryptofixyup-academy/understand-quickly/issues
- **Ask in discussions:** https://github.com/cryptofixyup-academy/understand-quickly/discussions
- **Check schema specs:** [`docs/spec/code-graph-protocol.md`](spec/code-graph-protocol.md)
- **Review examples:** Each supported format has integration docs in [`docs/integrations/`](integrations/)

---

## Contributing a fix

If you find a bug or want to improve the generator for your tool, contributions are welcome!

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the full guide.
