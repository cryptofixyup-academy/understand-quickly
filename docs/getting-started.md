# Getting Started

Choose your path based on what you're trying to do:

---

## 🤖 I'm an AI Agent / Developer

You want to **query code-knowledge graphs** to understand repositories.

### In 2 minutes:

```bash
# Fetch the registry
curl -fsSL https://looptech-ai.github.io/understand-quickly/registry.json | jq '.entries[] | select(.status == "ok") | {id, format, graph_url}' | head -20

# Pick an entry and fetch its graph
curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/.understand-anything/knowledge-graph.json | jq '.nodes[:5]'
```

### From code:

```javascript
// JavaScript: fetch registry, find a graph, use it
const res = await fetch('https://looptech-ai.github.io/understand-quickly/registry.json');
const { entries } = await res.json();
const entry = entries.find(e => e.id === 'nodejs/node' && e.status === 'ok');
const graph = await fetch(entry.graph_url).then(r => r.json());
console.log(`${graph.nodes.length} nodes in ${entry.id}`);
```

```python
# Python: same idea, with requests
import requests
registry = requests.get('https://looptech-ai.github.io/understand-quickly/registry.json').json()
entry = next(e for e in registry['entries'] if e['id'] == 'nodejs/node' and e['status'] == 'ok')
graph = requests.get(entry['graph_url']).json()
print(f"{len(graph['nodes'])} nodes in {entry['id']}")
```

### Best practices:

- **Cache the registry** — it updates once per day, so cache for 24 hours locally
- **Check status first** — only use entries with `status: "ok"`
- **Handle timeouts** — large graphs might take >30s to fetch
- **Graceful fallback** — if a graph is unavailable, skip it or use a backup

### Need more?

- See [`sdk-guide.md`](sdk-guide.md) for advanced patterns (batch fetching, error handling, format handling)
- See [`api-reference.md`](api-reference.md) for full entry schema
- Use the [MCP server](../mcp/README.md) for better caching if you're in Claude/Cursor/CodexNeeds to be indexed by an AI agent, you want to make your repository discoverable.

---

## 📚 I'm a Project Maintainer

You want to **register your repo** so AI tools can understand it instantly.

### Do you already have a code graph?

**Yes?** Jump to [Register an Existing Graph](#register-an-existing-graph).

**No?** Start with [Generate a Graph](#generate-a-graph).

---

### Generate a Graph

First, pick a tool that matches your codebase:

| Your repo | Best tool | One-liner |
|-----------|-----------|-----------|
| **General code** (any language) | [Understand-Anything](https://github.com/Lum1104/Understand-Anything) | `npx understand-anything-docker` |
| **Git history matters** | [GitNexus](https://github.com/abhigyanpatwari/GitNexus) | `npx gitnexus` |
| **Code review automation** | [code-review-graph](https://github.com/tirth8205/code-review-graph) | `npx code-review-graph` |
| **Whole-repo context** | [Repomix](https://github.com/yamadashy/repomix) or [gitingest](https://github.com/cyclotruc/gitingest) | `npx repomix` |
| **Custom format** | [Generic graph](../README.md#generic1) | See [code-graph-protocol](spec/code-graph-protocol.md) |

### Example: Using Understand-Anything

```bash
# Install and generate graph
npx understand-anything-docker

# Graph is created at: .understand-anything/knowledge-graph.json
# Commit it
git add .understand-anything/knowledge-graph.json
git commit -m "chore: add knowledge graph for AI agents"
git push
```

### Example: Using GitNexus

```bash
# Install
npm install -g gitnexus

# Generate graph (from repo root)
gitnexus

# Graph is at: .gitnexus/graph.json
git add .gitnexus/graph.json
git commit -m "chore: add knowledge graph for AI agents"
git push
```

---

### Register an Existing Graph

**You have a graph file.** Now register it. Pick the easiest:

#### Option 1: Wizard (Recommended — 2 minutes)

1. Go to https://looptech-ai.github.io/understand-quickly/add.html
2. Fill 4 fields:
   - **Repo:** `owner/repo` (e.g., `nodejs/node`)
   - **Graph URL:** URL to your `.json` file (e.g., `https://raw.githubusercontent.com/nodejs/node/main/.understand-anything/knowledge-graph.json`)
   - **Format:** Your graph format (e.g., `understand-anything@1`)
   - **Description:** One sentence about your project
3. **Submit** — bot opens the PR for you

#### Option 2: CLI (Automated — 1 minute)

```bash
cd /path/to/your/repo
npx @understand-quickly/cli add
```

The CLI auto-detects everything (repo owner, graph location, format). You just confirm.

#### Option 3: Manual PR (Advanced — 5 minutes)

1. Fork the repo
2. Edit `registry.json` and add your entry:
   ```json
   {
     "id": "owner/repo",
     "owner": "owner",
     "repo": "repo",
     "format": "understand-anything@1",
     "graph_url": "https://raw.githubusercontent.com/owner/repo/main/.understand-anything/knowledge-graph.json",
     "description": "Your project description"
   }
   ```
3. Commit and open a PR

---

### What happens next?

1. **CI validates** your entry (checks format, fetches graph, validates schema) — 5 minutes
2. **If you're a verified publisher:** PR auto-merges, graph is live immediately
3. **If you're first-time:** Maintainer reviews (usually 24–48 hours), then merges
4. **Nightly sync:** Registry sync checks your graph daily, updates status field
5. **Graph updates:** Whenever you commit a new graph, nightly sync picks it up

### Keep your graph fresh

Your graph file should **regenerate periodically** so it stays in sync with your code:

```bash
# Example: GitHub Actions (update graph nightly)
name: Update Knowledge Graph
on:
  schedule:
    - cron: '0 2 * * *'  # Every night at 2 UTC

jobs:
  update:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Generate graph
        run: npx understand-anything-docker
      - name: Commit and push
        run: |
          git config user.name "Bot"
          git config user.email "bot@example.com"
          git add .understand-anything/knowledge-graph.json
          git commit -m "chore: update knowledge graph" || exit 0
          git push
```

---

## 🔍 Troubleshooting

### "My entry won't validate"

Check [`troubleshooting.md`](troubleshooting.md#my-entry-wont-validate) for solutions.

### "My entry shows drift"

Your graph file is outdated. [`troubleshooting.md`](troubleshooting.md#my-entry-shows-drift) explains how to fix it.

### "I want to update/remove my entry"

See [`troubleshooting.md`](troubleshooting.md#i-want-to-update-my-entry-graph_url-description-tags-etc).

### Still stuck?

- Check existing issues: https://github.com/cryptofixyup-academy/understand-quickly/issues
- Ask a question: https://github.com/cryptofixyup-academy/understand-quickly/discussions
- Read the full FAQ: [`faq.md`](faq.md)

---

## 🚀 Next Steps

- **Ready to register?** Use the [wizard](https://looptech-ai.github.io/understand-quickly/add.html) or CLI
- **Building an AI tool?** See [`sdk-guide.md`](sdk-guide.md) for consuming the registry
- **Deep dive?** Read [`api-reference.md`](api-reference.md) and [`spec/code-graph-protocol.md`](spec/code-graph-protocol.md)
- **Integration guide?** Check out [`cookbook.md`](cookbook.md) (coming soon)
