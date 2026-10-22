/**
 * 前端内存缓存模块
 * 基于 Map 的 TTL 缓存，超限淘汰最旧条目，提供 withCache 读穿与按前缀删除
 */
import { logDebug } from '../utils/logger';

interface CacheEntry<T> {
  data: T;
  timestamp: number;
  expiresAt: number;
}

/** 未命中哨兵；用符号而非 null，才能区分「缓存了 null」和「没缓存」 */
export const CACHE_MISS = Symbol('CACHE_MISS');

class MemoryCache {
  private cache = new Map<string, CacheEntry<unknown>>();
  private defaultTTL = 5 * 60 * 1000;
  private maxSize = 200;

  /**
   * 淘汰最久未使用的一条。Map 按插入顺序迭代，而命中时总是先 delete 再 set（见 get/set），
   * 所以「第一个 key」就是最久未用的那条，不必 O(n) 扫全表找最小 timestamp
   */
  private evictOldest(): void {
    const oldestKey = this.cache.keys().next();
    if (!oldestKey.done) {
      this.cache.delete(oldestKey.value);
    }
  }

  get<T>(key: string): T | typeof CACHE_MISS {
    const entry = this.cache.get(key);

    if (!entry) {
      return CACHE_MISS;
    }

    if (Date.now() > entry.expiresAt) {
      this.cache.delete(key);
      return CACHE_MISS;
    }

    entry.timestamp = Date.now();

    this.cache.delete(key);
    this.cache.set(key, entry);

    return entry.data as T;
  }

  /** 未传 ttl 时默认 5 分钟 */
  set<T>(key: string, data: T, ttl?: number): void {
    if (this.cache.size >= this.maxSize) {
      this.evictOldest();
    }
    const now = Date.now();
    // 先删再插：覆盖已有 key 时也要把它挪到末尾，否则插入顺序不等于使用顺序
    this.cache.delete(key);
    this.cache.set(key, {
      data,
      timestamp: now,
      expiresAt: now + (ttl ?? this.defaultTTL),
    });
  }

  delete(key: string): boolean {
    return this.cache.delete(key);
  }

  deleteByPrefix(prefix: string): number {
    let count = 0;
    for (const key of this.cache.keys()) {
      if (key.startsWith(prefix)) {
        this.cache.delete(key);
        count++;
      }
    }
    return count;
  }

  clear(): void {
    this.cache.clear();
  }

  cleanup(): number {
    const now = Date.now();
    let count = 0;

    for (const [key, entry] of this.cache.entries()) {
      if (now > entry.expiresAt) {
        this.cache.delete(key);
        count++;
      }
    }

    return count;
  }

  get size(): number {
    return this.cache.size;
  }
}

export const cache = new MemoryCache();

export const CacheKeys = {
  VERSIONS: 'versions',
  JAVA: 'java',
  LOADERS: 'loaders',
  CONFIG: 'config',
} as const;

export async function withCache<T>(key: string, fetcher: () => Promise<T>, ttl?: number): Promise<T> {
  const cached = cache.get<T>(key);
  if (cached !== CACHE_MISS) {
    return cached;
  }

  const data = await fetcher();
  cache.set(key, data, ttl);

  return data;
}

if (typeof window !== 'undefined') {
  setInterval(
    () => {
      const cleaned = cache.cleanup();
      if (cleaned > 0) {
        logDebug(`Cleaned ${cleaned} expired entries`, 'Cache');
      }
    },
    5 * 60 * 1000,
  );
}
