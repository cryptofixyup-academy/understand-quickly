// Bounded TTL cache — prevents the unbounded RSS growth that occurs when
// module-level Maps accumulate entries across the process lifetime.
//
// This is the memory-safe replacement for the pattern in registry.ts where
// unbounded Maps are used for registry and stats caching. At 10M+ requests
// per day an unbounded cache will steadily consume RSS until the container
// is OOM-killed (see: Next.js 15/16 standalone OOM paradox, section 7.2).
//
// Usage (drop-in for registry.ts cache Maps):
//   const cache = new BoundedTtlCache<CacheRecord>({ maxSize: 256, ttlMs: 60_000 });
//   const record = cache.get(cacheKey, Date.now());
//   cache.set(cacheKey, { fetchedAt: now, registry: body }, Date.now());

export interface TtlEntry<V> {
  value: V;
  fetchedAt: number;
}

export class BoundedTtlCache<V> {
  private readonly map = new Map<string, TtlEntry<V>>();
  private readonly maxSize: number;
  private readonly ttlMs: number;

  constructor(opts: { maxSize: number; ttlMs: number }) {
    this.maxSize = opts.maxSize;
    this.ttlMs = opts.ttlMs;
  }

  /** Returns the cached value if present and not expired; undefined otherwise. */
  get(key: string, now = Date.now()): V | undefined {
    const entry = this.map.get(key);
    if (!entry) return undefined;
    if (now - entry.fetchedAt >= this.ttlMs) {
      this.map.delete(key);
      return undefined;
    }
    return entry.value;
  }

  /** Stores a value. Evicts the oldest entry when maxSize is reached. */
  set(key: string, value: V, now = Date.now()): void {
    if (!this.map.has(key) && this.map.size >= this.maxSize) {
      const oldest = this.map.keys().next().value;
      if (oldest !== undefined) this.map.delete(oldest);
    }
    this.map.set(key, { value, fetchedAt: now });
  }

  /** Returns the raw entry including fetchedAt, or undefined if absent/expired. */
  getEntry(key: string, now = Date.now()): TtlEntry<V> | undefined {
    const entry = this.map.get(key);
    if (!entry) return undefined;
    if (now - entry.fetchedAt >= this.ttlMs) {
      this.map.delete(key);
      return undefined;
    }
    return entry;
  }

  delete(key: string): boolean {
    return this.map.delete(key);
  }

  clear(): void {
    this.map.clear();
  }

  get size(): number {
    return this.map.size;
  }

  /** Evicts all entries that have exceeded ttlMs. Useful for periodic cleanup. */
  evictExpired(now = Date.now()): number {
    let evicted = 0;
    for (const [key, entry] of this.map) {
      if (now - entry.fetchedAt >= this.ttlMs) {
        this.map.delete(key);
        evicted++;
      }
    }
    return evicted;
  }
}
