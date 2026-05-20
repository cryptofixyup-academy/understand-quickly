import { test } from 'node:test';
import assert from 'node:assert/strict';
import { extractStats, extractSourceSha, validateBodyLimits, __internal } from '../extract.mjs';

const { MAX_NODES, MAX_EDGES, MAX_LABEL_LEN, MAX_TREE_DEPTH, maxDepth } = __internal;

// ---------------------------------------------------------------------------
// extractStats
// ---------------------------------------------------------------------------

test('extractStats: understand-anything@1 counts nodes and edges', () => {
  const body = {
    nodes: [
      { id: 'n1', kind: 'file', label: 'a' },
      { id: 'n2', kind: 'function', label: 'b' }
    ],
    edges: [{ from: 'n1', to: 'n2', kind: 'contains' }]
  };
  const s = extractStats('understand-anything@1', body);
  assert.equal(s.nodes_count, 2);
  assert.equal(s.edges_count, 1);
  assert.deepEqual(s.top_kinds, [
    { kind: 'file', count: 1 },
    { kind: 'function', count: 1 }
  ]);
  assert.deepEqual(s.languages, []);
});

test('extractStats: gitnexus@1 counts graph.nodes and graph.links', () => {
  const body = {
    graph: {
      nodes: [
        { id: 'a', label: 'File', properties: { language: 'typescript' } },
        { id: 'b', label: 'Function', properties: { language: 'typescript' } },
        { id: 'c', label: 'File', properties: {} }
      ],
      links: [{ id: 'r1', source: 'a', target: 'b', type: 'DEFINES' }]
    },
    metadata: { languages: ['python'] }
  };
  const s = extractStats('gitnexus@1', body);
  assert.equal(s.nodes_count, 3);
  assert.equal(s.edges_count, 1);
  assert.equal(s.top_kinds[0].kind, 'file');
  assert.equal(s.top_kinds[0].count, 2);
  assert.ok(s.languages.includes('typescript'));
  assert.ok(s.languages.includes('python'));
});

test('extractStats: code-review-graph@1 uses stats.languages array', () => {
  const body = {
    nodes: [
      { id: 1, kind: 'File' },
      { id: 2, kind: 'Function' }
    ],
    edges: [],
    stats: { nodes_by_kind: {}, languages: ['python', 'python', 'javascript'] }
  };
  const s = extractStats('code-review-graph@1', body);
  assert.equal(s.nodes_count, 2);
  assert.equal(s.edges_count, 0);
  assert.ok(s.languages.includes('python'));
  assert.ok(s.languages.includes('javascript'));
});

test('extractStats: code-review-graph@1 handles languages as object keys', () => {
  const body = {
    nodes: [],
    edges: [],
    stats: { nodes_by_kind: {}, languages: { python: 10, javascript: 5 } }
  };
  const s = extractStats('code-review-graph@1', body);
  assert.ok(s.languages.includes('python'));
  assert.ok(s.languages.includes('javascript'));
});

test('extractStats: generic@1 returns node/edge counts with empty kinds and languages', () => {
  const body = { nodes: [{ id: 'a' }], edges: [{ source: 'a', target: 'a' }] };
  const s = extractStats('generic@1', body);
  assert.equal(s.nodes_count, 1);
  assert.equal(s.edges_count, 1);
  assert.deepEqual(s.top_kinds, []);
  assert.deepEqual(s.languages, []);
});

test('extractStats: bundle@1 uses manifest.file_count if present', () => {
  const body = { manifest: { file_count: 42 }, files: [{ path: 'a' }] };
  const s = extractStats('bundle@1', body);
  assert.equal(s.nodes_count, 42);
  assert.equal(s.edges_count, 0);
  assert.deepEqual(s.top_kinds, [{ kind: 'file', count: 42 }]);
});

test('extractStats: bundle@1 falls back to files.length when no manifest.file_count', () => {
  const body = { manifest: {}, files: [{ path: 'a' }, { path: 'b' }] };
  const s = extractStats('bundle@1', body);
  assert.equal(s.nodes_count, 2);
});

test('extractStats: bundle@1 returns 0 counts for empty body', () => {
  const s = extractStats('bundle@1', {});
  assert.equal(s.nodes_count, 0);
  assert.deepEqual(s.top_kinds, []);
});

test('extractStats: unknown format returns empty object', () => {
  const s = extractStats('future-format@99', { nodes: [], edges: [] });
  assert.deepEqual(s, {});
});

test('extractStats: does not throw on malformed body', () => {
  assert.doesNotThrow(() => extractStats('understand-anything@1', null));
  assert.doesNotThrow(() => extractStats('gitnexus@1', { graph: null }));
  assert.doesNotThrow(() => extractStats('code-review-graph@1', { stats: null }));
});

// ---------------------------------------------------------------------------
// extractSourceSha
// ---------------------------------------------------------------------------

const VALID_SHA = 'a'.repeat(40);

test('extractSourceSha: understand-anything@1 reads metadata.source_sha', () => {
  const body = { metadata: { source_sha: VALID_SHA } };
  assert.equal(extractSourceSha('understand-anything@1', body), VALID_SHA);
});

test('extractSourceSha: understand-anything@1 falls back to metadata.commit', () => {
  const body = { metadata: { commit: VALID_SHA } };
  assert.equal(extractSourceSha('understand-anything@1', body), VALID_SHA);
});

test('extractSourceSha: gitnexus@1 reads metadata.commit', () => {
  const body = { metadata: { commit: VALID_SHA } };
  assert.equal(extractSourceSha('gitnexus@1', body), VALID_SHA);
});

test('extractSourceSha: gitnexus@1 reads graph.metadata.commit as fallback', () => {
  const body = { graph: { metadata: { commit: VALID_SHA } } };
  assert.equal(extractSourceSha('gitnexus@1', body), VALID_SHA);
});

test('extractSourceSha: code-review-graph@1 reads metadata.commit', () => {
  const body = { metadata: { commit: VALID_SHA } };
  assert.equal(extractSourceSha('code-review-graph@1', body), VALID_SHA);
});

test('extractSourceSha: bundle@1 reads manifest.commit', () => {
  const body = { manifest: { commit: VALID_SHA } };
  assert.equal(extractSourceSha('bundle@1', body), VALID_SHA);
});

test('extractSourceSha: bundle@1 falls back to metadata.commit', () => {
  const body = { metadata: { commit: VALID_SHA } };
  assert.equal(extractSourceSha('bundle@1', body), VALID_SHA);
});

test('extractSourceSha: rejects short sha', () => {
  const body = { metadata: { commit: 'abc123' } };
  assert.equal(extractSourceSha('understand-anything@1', body), null);
});

test('extractSourceSha: rejects branch name', () => {
  const body = { metadata: { commit: 'main' } };
  assert.equal(extractSourceSha('gitnexus@1', body), null);
});

test('extractSourceSha: generic@1 always returns null', () => {
  const body = { metadata: { commit: VALID_SHA } };
  assert.equal(extractSourceSha('generic@1', body), null);
});

test('extractSourceSha: returns null for null body', () => {
  assert.equal(extractSourceSha('understand-anything@1', null), null);
});

// ---------------------------------------------------------------------------
// validateBodyLimits
// ---------------------------------------------------------------------------

test('validateBodyLimits: valid body passes', () => {
  const body = { nodes: [{ id: 'n1', label: 'a' }], edges: [] };
  assert.deepEqual(validateBodyLimits(body, 'understand-anything@1'), { ok: true });
});

test('validateBodyLimits: null/primitive body passes (not a graph)', () => {
  assert.deepEqual(validateBodyLimits(null, 'generic@1'), { ok: true });
  assert.deepEqual(validateBodyLimits('string', 'generic@1'), { ok: true });
});

test('validateBodyLimits: too many nodes returns oversize', () => {
  const body = { nodes: new Array(MAX_NODES + 1).fill({ id: 'x' }), edges: [] };
  const r = validateBodyLimits(body, 'understand-anything@1');
  assert.equal(r.ok, false);
  assert.equal(r.status, 'oversize');
  assert.match(r.error, /nodes/);
});

test('validateBodyLimits: too many edges returns oversize', () => {
  const body = { nodes: [], edges: new Array(MAX_EDGES + 1).fill({ source: 'a', target: 'b' }) };
  const r = validateBodyLimits(body, 'understand-anything@1');
  assert.equal(r.ok, false);
  assert.equal(r.status, 'oversize');
  assert.match(r.error, /edges/);
});

test('validateBodyLimits: label exceeding MAX_LABEL_LEN returns invalid', () => {
  const longLabel = 'x'.repeat(MAX_LABEL_LEN + 1);
  const body = { nodes: [{ id: 'n1', label: longLabel }], edges: [] };
  const r = validateBodyLimits(body, 'understand-anything@1');
  assert.equal(r.ok, false);
  assert.equal(r.status, 'invalid');
  assert.match(r.error, /label/);
});

test('validateBodyLimits: deeply nested object (schema bomb) returns invalid', () => {
  let nested = {};
  let cur = nested;
  for (let i = 0; i < MAX_TREE_DEPTH + 2; i++) {
    cur.child = {};
    cur = cur.child;
  }
  const r = validateBodyLimits(nested, 'generic@1');
  assert.equal(r.ok, false);
  assert.equal(r.status, 'invalid');
  assert.match(r.error, /schema bomb/);
});

test('validateBodyLimits: gitnexus@1 checks graph.nodes and graph.links', () => {
  const body = {
    graph: {
      nodes: new Array(MAX_NODES + 1).fill({ id: 'x', label: 'File' }),
      links: []
    }
  };
  const r = validateBodyLimits(body, 'gitnexus@1');
  assert.equal(r.ok, false);
  assert.equal(r.status, 'oversize');
});

// ---------------------------------------------------------------------------
// maxDepth internal helper
// ---------------------------------------------------------------------------

test('maxDepth: flat object is depth 1', () => {
  assert.equal(maxDepth({ a: 1, b: 2 }), 1);
});

test('maxDepth: one level nested is depth 2', () => {
  assert.equal(maxDepth({ a: { b: 1 } }), 2);
});

test('maxDepth: array nesting counts', () => {
  assert.equal(maxDepth([[1, 2], [3]]), 2);
});

test('maxDepth: stops at cap', () => {
  let nested = {};
  let cur = nested;
  for (let i = 0; i < 50; i++) { cur.c = {}; cur = cur.c; }
  assert.ok(maxDepth(nested, 10) > 10);
});

test('maxDepth: null/primitive returns 1', () => {
  assert.equal(maxDepth(null), 1);
  assert.equal(maxDepth(42), 1);
  assert.equal(maxDepth('str'), 1);
});
