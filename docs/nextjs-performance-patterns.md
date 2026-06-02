# Next.js 15+ High-Performance Architecture Patterns

Reference guide for the 2026 SOTA stack: 10M+ concurrent users at sub-100ms p99 latency.

---

## 1. Rendering: Partial Prerendering (PPR)

PPR serves the static shell from a global edge cache while streaming dynamic segments. Cache Components allow granular per-function caching via `'use cache'`.

```typescript
import { cacheLife } from 'next/cache';

export async function fetchHighPerformanceData(params: string) {
  'use cache';
  cacheLife('minutes'); // stale: 60s, revalidate: 120s, expire: 3600s

  const data = await db.query('SELECT * FROM metrics WHERE id = ?', [params]);
  return data;
}
```

**next.config.ts** (minimum viable PPR config):

```typescript
const nextConfig: NextConfig = {
  experimental: { ppr: true, dynamicIO: true },
  output: 'standalone',
};
```

---

## 2. Validation: Typia over Zod

| Library | Throughput | Bundle | Mechanism |
| :-- | :-: | :-: | :-- |
| Zod | ~100k ops/sec | ~13 KB | Runtime interpreter |
| @zod/mini | ~500k ops/sec | ~5.5 KB | Lighter interpreter |
| Typia | ~10.5M ops/sec | ~0 KB | Build-time compiled JS |

**Setup** (`ts-patch` required):

```bash
npm install typia ts-patch
npx ts-patch install
```

`tsconfig.json`:
```json
{ "plugins": [{ "transform": "typia/lib/transform" }] }
```

**Usage**:

```typescript
import typia, { tags } from 'typia';

interface MetricQuery {
  metric: string & tags.Pattern<'^[a-z_]+$'>;
  limit?: number & tags.Minimum<1> & tags.Maximum<1000>;
}

// Replaced with optimized JS at build time — no runtime overhead
export const assertMetricQuery = typia.createAssert<MetricQuery>();
```

---

## 3. Compute: WebGPU

WebGPU reached universal browser support in early 2026. Key advantages over WebGL:

- **1,000,000+ particles** vs WebGL ceiling of ~50,000 (20x)
- **Render Bundles** pre-record draw calls, replaying 1M objects in a single call
- **Storage buffers** indexed by `instance_index` eliminate per-object bind overhead
- **Multi-threaded command buffers** vs WebGL's sequential CPU-bound translation

**Critical cleanup** (prevents GPU memory leaks):

```typescript
return () => {
  cancelAnimationFrame(frameRef.current);
  storageBuffer.destroy();  // Must be explicit
  device.destroy();
};
```

See `nextjs-performance/components/WebGpuParticles.tsx` for the full 1M-particle implementation.

---

## 4. Synchronization: Delta-CRDTs

| CRDT Type | Payload | Fault Tolerance |
| :-- | :-: | :-: |
| State-based | Full state (O(nodes)) | High |
| Operational | Per-op (O(1)) | Requires exactly-once delivery |
| Delta-based | State diffs (O(changed)) | High |

Delta-CRDTs are the production default. Paired with **VCube-PS** (virtual hypercube)
for logarithmic causal delivery at p99 < 50ms globally.

```typescript
// Increment returns the updated counter AND a minimal delta for transmission
const [nextCounter, delta] = increment(counter);
await broadcast(serializeDelta(delta)); // Only changed entries sent

// On receiving node
const updated = applyDelta(remoteCounter, deserializeDelta(payload));
```

See `nextjs-performance/lib/delta-crdt.ts` for the full G-Counter implementation.

---

## 5. The RSC Serialization Gotcha — "Double Data"

**Problem**: RSC Flight protocol transmits HTML (for initial paint) **plus** line-delimited JSON (for client-router reconciliation). A shop shell serializes ~15KB of redundant layout per request. At 50 prefetches/session: ~1MB wasted per user. Under 1,000+ RPS response times climb from 17ms to 8,000+ms.

**Solution**: A strict Data Access Layer (DAL) with `import 'server-only'`:

```typescript
import 'server-only'; // Build-time guard — fails if imported from client bundle

type DbRow = { id: string; value: number; timestamp: string; metadata: object; auditLog: string[] };
type MetricRow = Pick<DbRow, 'id' | 'value' | 'timestamp'>;

export async function getMetricsForUser(userId: string, query: unknown): Promise<MetricRow[]> {
  const parsed = assertMetricQuery(query);
  const rows = await db.query('SELECT * FROM metrics WHERE user_id = ?', [userId]);

  // Explicit destructure: only 3 fields cross the wire, not all 8
  return rows.map(({ id, value, timestamp }) => ({ id, value, timestamp }));
}
```

---

## 6. The OOM Paradox — Standalone Memory Leaks

**Problem**: Next.js 15/16 in `output: 'standalone'` exhibits steady RSS growth:
- `undici` retains performance entries past GC
- Turbopack RSS spikes to 12+GB on large route graphs
- Module-level state (unbounded Maps, Zustand stores) never frees

**Solution**: explicit GC flags + health endpoint:

```bash
# package.json start script:
node --expose-gc --max-old-space-size=2048 server.js
```

```typescript
// /api/health — called by Kubernetes liveness probe
export async function GET() {
  const { rss, heapUsed } = process.memoryUsage();
  if (rss > 1.5 * 1024**3 && typeof global.gc === 'function') {
    global.gc(); // blocking stop-the-world — low-traffic windows only
  }
  const status = rss > 1.8 * 1024**3 ? 503 : 200;
  return NextResponse.json({ rssGb: (rss / 1024**3).toFixed(2) }, { status });
}
```

For this MCP server, `mcp/src/bounded-cache.ts` provides a drop-in bounded replacement
for the unbounded `Map` caches in `registry.ts`:

```typescript
// Before (unbounded — grows until process restart):
const cache = new Map<string, CacheRecord>();

// After (bounded — evicts oldest when maxSize is reached):
const cache = new BoundedTtlCache<CacheRecord>({ maxSize: 256, ttlMs: 60_000 });
```

---

## 7. Zero-Trust Middleware — CVE-2025-29927

**Problem**: CVE-2025-29927 (fixed in Next.js ≥15.2.3) demonstrated that the
`x-middleware-subrequest` header could be forged to bypass middleware entirely.
Centralizing auth at the edge means a single forged header collapses all security.

**Correct layering**:

| Concern | Where |
| :-- | :-- |
| Routing, geo-redirect, rate-limit hints | Middleware |
| Auth / authz / row-level security | Server Components + Server Actions |
| Sensitive credential modules | `import 'server-only'` |

```typescript
// middleware.ts — additive headers ONLY, no auth logic
export function middleware(request: NextRequest) {
  const response = NextResponse.next();
  response.headers.set('X-Frame-Options', 'DENY');
  response.headers.set('X-Content-Type-Options', 'nosniff');
  return response;
}

// app/dashboard/page.tsx — auth checked here, inside the trust boundary
import 'server-only';
import { verifySession } from '@/lib/server/auth';

export default async function DashboardPage() {
  const session = await verifySession(); // throws if unauthorized
  // ...
}
```

---

## Performance Targets Summary

| Dimension | SOTA Target (2026) |
| :-- | :-: |
| Initial Page Shell (TTFB) | < 10 ms |
| Client-side data parallelism | 100x vs WebGL |
| Global sync (p99) | < 50 ms |
| Validation throughput | ~10M ops/sec |

---

## Implementation

Working TypeScript implementation of all patterns: [`cryptofixyup/clear-code/nextjs-performance/`](https://github.com/cryptofixyup/clear-code/tree/main/nextjs-performance)
