# SDK Guide: Consuming the Registry Programmatically

This guide covers how to consume understand-quickly from your code without using MCP.

## Quick Start

### JavaScript/TypeScript

```javascript
// Simple fetch-and-filter
async function getGraphsForLanguage(language) {
  const res = await fetch('https://looptech-ai.github.io/understand-quickly/registry.json');
  const { entries } = await res.json();
  
  return entries
    .filter(e => e.status === 'ok')
    .filter(e => e.tags?.includes(language))
    .map(e => ({ id: e.id, format: e.format, url: e.graph_url }));
}

// Fetch and parse a graph
async function fetchGraph(graphUrl) {
  const res = await fetch(graphUrl);
  if (!res.ok) throw new Error(`Failed to fetch: ${res.status}`);
  return res.json();
}

// Example usage
const graphs = await getGraphsForLanguage('python');
for (const { id, url } of graphs) {
  const graph = await fetchGraph(url);
  console.log(`${id}: ${graph.nodes.length} nodes`);
}
```

### Python

```python
import requests
from typing import Optional

def get_registry() -> dict:
    """Fetch the latest registry."""
    resp = requests.get(
        'https://looptech-ai.github.io/understand-quickly/registry.json'
    )
    resp.raise_for_status()
    return resp.json()

def get_ok_entries(format_prefix: Optional[str] = None) -> list:
    """Get all "ok" entries, optionally filtered by format."""
    registry = get_registry()
    entries = [e for e in registry['entries'] if e['status'] == 'ok']
    
    if format_prefix:
        entries = [e for e in entries if e['format'].startswith(format_prefix)]
    
    return entries

def fetch_graph(graph_url: str) -> dict:
    """Fetch and parse a knowledge graph."""
    resp = requests.get(graph_url)
    resp.raise_for_status()
    return resp.json()

# Example usage
entries = get_ok_entries('understand-anything')
for entry in entries[:3]:
    graph = fetch_graph(entry['graph_url'])
    print(f"{entry['id']}: {len(graph['nodes'])} nodes")
```

### Go

```go
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
)

type Entry struct {
	ID       string `json:"id"`
	Format   string `json:"format"`
	GraphURL string `json:"graph_url"`
	Status   string `json:"status"`
	Tags     []string `json:"tags"`
}

type Registry struct {
	Entries []Entry `json:"entries"`
}

func GetRegistry() (*Registry, error) {
	resp, err := http.Get(
		"https://looptech-ai.github.io/understand-quickly/registry.json",
	)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}

	var reg Registry
	if err := json.Unmarshal(data, &reg); err != nil {
		return nil, err
	}
	return &reg, nil
}

func GetOkEntries(formatPrefix string) []Entry {
	reg, _ := GetRegistry()
	var result []Entry
	for _, e := range reg.Entries {
		if e.Status == "ok" && strings.HasPrefix(e.Format, formatPrefix) {
			result = append(result, e)
		}
	}
	return result
}

func main() {
	entries := GetOkEntries("understand-anything")
	for _, e := range entries {
		fmt.Printf("%s: %s\n", e.ID, e.GraphURL)
	}
}
```

---

## Patterns

### Caching

Since the registry updates only nightly, cache it locally:

```javascript
// In-memory cache with 1-hour TTL
let cachedRegistry = null;
let cacheExpiry = 0;

async function getCachedRegistry() {
  const now = Date.now();
  if (cachedRegistry && now < cacheExpiry) {
    return cachedRegistry;
  }

  const res = await fetch('https://looptech-ai.github.io/understand-quickly/registry.json');
  cachedRegistry = await res.json();
  cacheExpiry = now + 3600 * 1000; // 1 hour
  return cachedRegistry;
}
```

### Error Handling

Always handle network failures and invalid graphs:

```javascript
async function safeGetGraph(graphUrl, timeout = 30000) {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort(), timeout);

  try {
    const res = await fetch(graphUrl, { signal: controller.signal });
    if (!res.ok) {
      if (res.status === 404) throw new Error('Graph not found (404)');
      throw new Error(`HTTP ${res.status}`);
    }
    return await res.json();
  } catch (err) {
    if (err.name === 'AbortError') throw new Error(`Timeout after ${timeout}ms`);
    throw err;
  } finally {
    clearTimeout(id);
  }
}
```

### Batch Processing

For large-scale analysis, fetch and cache multiple graphs in parallel:

```javascript
async function batchFetchGraphs(entries, concurrency = 5) {
  const results = [];
  const queue = [...entries];

  async function worker() {
    while (queue.length > 0) {
      const entry = queue.shift();
      try {
        const graph = await safeGetGraph(entry.graph_url, 30000);
        results.push({ entry, graph, error: null });
      } catch (error) {
        results.push({ entry, graph: null, error: error.message });
      }
    }
  }

  const workers = Array(concurrency).fill(null).map(() => worker());
  await Promise.all(workers);
  return results;
}
```

### Searching Across Graphs

For simple substring searches, fan out across multiple graphs:

```python
def search_across_graphs(query: str, max_results: int = 50) -> list:
    """Search for a term across all graphs."""
    results = []
    entries = get_ok_entries()
    
    for entry in entries[:10]:  # Limit to first 10 to avoid timeouts
        try:
            graph = fetch_graph(entry['graph_url'])
            
            # Search in nodes
            for node in graph.get('nodes', []):
                if query.lower() in str(node).lower():
                    results.append({
                        'repo': entry['id'],
                        'node': node,
                        'type': 'node'
                    })
                    if len(results) >= max_results:
                        return results
            
            # Search in edges
            for edge in graph.get('edges', []):
                if query.lower() in str(edge).lower():
                    results.append({
                        'repo': entry['id'],
                        'edge': edge,
                        'type': 'edge'
                    })
                    if len(results) >= max_results:
                        return results
                        
        except Exception as e:
            print(f"Failed to fetch {entry['id']}: {e}")
            continue
    
    return results
```

---

## Working with Different Formats

Each format has a different node/edge structure. Here's how to handle multiple formats:

```javascript
function getNodes(graph, format) {
  // understand-anything@1: `nodes` array
  if (graph.nodes) return graph.nodes;
  
  // gitnexus@1: `graph.nodes`
  if (graph.graph?.nodes) return graph.graph.nodes;
  
  // code-review-graph@1: `nodes` array (File/Class/Function)
  if (format.startsWith('code-review-graph')) return graph.nodes;
  
  // generic@1: `nodes` array
  return graph.nodes || [];
}

function getEdges(graph, format) {
  // understand-anything@1: `edges` array
  if (graph.edges) return graph.edges;
  
  // gitnexus@1: `graph.links`
  if (graph.graph?.links) return graph.graph.links;
  
  // code-review-graph@1: `edges` array
  if (graph.edges) return graph.edges;
  
  // generic@1: `edges` array
  return graph.edges || [];
}

// Usage
for (const entry of entries) {
  const graph = await fetchGraph(entry.graph_url);
  const nodes = getNodes(graph, entry.format);
  const edges = getEdges(graph, entry.format);
  
  console.log(`${entry.id}: ${nodes.length} nodes, ${edges.length} edges`);
}
```

---

## Using Stats for Quick Insights

For analytics or dashboards, use pre-aggregated `stats.json` instead of fetching all graphs:

```javascript
async function getTopConcepts(limit = 10) {
  const res = await fetch('https://looptech-ai.github.io/understand-quickly/stats.json');
  const stats = await res.json();
  
  return stats.concepts
    .sort((a, b) => b.frequency - a.frequency)
    .slice(0, limit)
    .map(c => ({ name: c.name, frequency: c.frequency, repos: c.sample_repos.length }));
}

async function getFormatDistribution() {
  const res = await fetch('https://looptech-ai.github.io/understand-quickly/stats.json');
  const stats = await res.json();
  
  return Object.entries(stats.entries_by_format)
    .sort(([, a], [, b]) => b - a)
    .map(([format, count]) => ({ format, count }));
}

// Usage
const concepts = await getTopConcepts();
console.table(concepts);
```

---

## Rate Limiting & Quotas

- **Registry file**: ~immutable; cache for up to 24 hours
- **Graph files**: Cache according to provider's `Cache-Control` headers
- **Concurrent fetches**: Start with 5-10 concurrent requests; throttle if you see 429/503
- **GitHub API** (if checking drift): 60 requests/hour unauthenticated

---

## Schema Reference

See [`api-reference.md`](api-reference.md) for the full entry/registry schema.

See [`spec/code-graph-protocol.md`](spec/code-graph-protocol.md) for graph format specifications.

---

## Next Steps

- **For AI agents:** Use the [MCP server](../mcp/README.md) for better caching and error handling
- **For analytics:** Use the [CLI](../cli/README.md) to batch-process graphs locally
- **For custom integrations:** Check out the format specs in [`docs/spec/`](spec/)
