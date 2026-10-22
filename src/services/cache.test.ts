import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { cache, withCache, CacheKeys, CACHE_MISS } from './cache';

describe('MemoryCache', () => {
  beforeEach(() => {
    cache.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('初始大小为 0', () => {
    expect(cache.size).toBe(0);
  });

  it('set 和 get 基本操作', () => {
    cache.set('key1', 'value1');
    expect(cache.get('key1')).toBe('value1');
    expect(cache.size).toBe(1);
  });

  it('get 不存在的 key 返回 CACHE_MISS', () => {
    expect(cache.get('nonexistent')).toBe(CACHE_MISS);
  });

  it('缓存 null 值仍算命中，不会退回 CACHE_MISS', () => {
    cache.set('nullable', null);
    expect(cache.get('nullable')).toBeNull();
  });

  it('支持复杂数据类型', () => {
    const data = { name: 'test', items: [1, 2, 3] };
    cache.set('complex', data);
    expect(cache.get('complex')).toEqual(data);
  });

  it('覆盖已有 key', () => {
    cache.set('key', 'old');
    cache.set('key', 'new');
    expect(cache.get('key')).toBe('new');
    expect(cache.size).toBe(1);
  });

  it('默认 TTL 5 分钟后过期', () => {
    cache.set('expire', 'data');
    expect(cache.get('expire')).toBe('data');

    vi.advanceTimersByTime(5 * 60 * 1000 + 1);
    expect(cache.get('expire')).toBe(CACHE_MISS);
  });

  it('自定义 TTL', () => {
    cache.set('short', 'data', 1000);
    expect(cache.get('short')).toBe('data');

    vi.advanceTimersByTime(500);
    expect(cache.get('short')).toBe('data');

    vi.advanceTimersByTime(501);
    expect(cache.get('short')).toBe(CACHE_MISS);
  });

  it('过期的 entry 被 get 时自动删除', () => {
    cache.set('auto-del', 'data', 100);
    vi.advanceTimersByTime(101);

    expect(cache.size).toBe(1);
    cache.get('auto-del');
    expect(cache.size).toBe(0);
  });

  it('delete 删除指定 key', () => {
    cache.set('a', 1);
    cache.set('b', 2);
    expect(cache.delete('a')).toBe(true);
    expect(cache.get('a')).toBe(CACHE_MISS);
    expect(cache.get('b')).toBe(2);
  });

  it('delete 不存在的 key 返回 false', () => {
    expect(cache.delete('nonexistent')).toBe(false);
  });

  it('deleteByPrefix 删除匹配前缀的所有 key', () => {
    cache.set('loader:forge', 'data1');
    cache.set('loader:fabric', 'data2');
    cache.set('versions:1.20', 'data3');

    const count = cache.deleteByPrefix('loader');
    expect(count).toBe(2);
    expect(cache.get('loader:forge')).toBe(CACHE_MISS);
    expect(cache.get('loader:fabric')).toBe(CACHE_MISS);
    expect(cache.get('versions:1.20')).toBe('data3');
  });

  it('deleteByPrefix 无匹配时返回 0', () => {
    cache.set('key', 'value');
    expect(cache.deleteByPrefix('nonexistent')).toBe(0);
  });

  it('clear 清除所有缓存', () => {
    cache.set('a', 1);
    cache.set('b', 2);
    cache.set('c', 3);
    cache.clear();
    expect(cache.size).toBe(0);
    expect(cache.get('a')).toBe(CACHE_MISS);
  });

  it('cleanup 只清除过期的 entry', () => {
    cache.set('expired1', 'data', 100);
    cache.set('expired2', 'data', 200);
    cache.set('alive', 'data', 10_000);

    vi.advanceTimersByTime(250);

    const cleaned = cache.cleanup();
    expect(cleaned).toBe(2);
    expect(cache.get('alive')).toBe('data');
    expect(cache.size).toBe(1);
  });

  it('cleanup 无过期 entry 返回 0', () => {
    cache.set('a', 1);
    expect(cache.cleanup()).toBe(0);
  });

  it('超过 maxSize 时淘汰最久未使用的一条', () => {
    // maxSize 是 200，塞满后再加一条会触发淘汰
    for (let i = 0; i < 200; i += 1) {
      cache.set(`k${i}`, i);
    }
    expect(cache.size).toBe(200);

    expect(cache.get('k0')).toBe(0);

    cache.set('newest', 'x');

    expect(cache.get('k0')).toBe(0);
    expect(cache.get('k1')).toBe(CACHE_MISS);
    expect(cache.get('newest')).toBe('x');
    expect(cache.size).toBe(200);
  });

  it('覆盖已有 key 也会把它当成最近使用', () => {
    for (let i = 0; i < 200; i += 1) {
      cache.set(`k${i}`, i);
    }

    cache.set('k0', 'updated');
    cache.set('newest', 'x');

    expect(cache.get('k0')).toBe('updated');
    expect(cache.get('k1')).toBe(CACHE_MISS);
  });
});

describe('withCache', () => {
  beforeEach(() => {
    cache.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('首次调用执行 fetcher 并缓存结果', async () => {
    const fetcher = vi.fn().mockResolvedValue('result');

    const result = await withCache('test-key', fetcher);
    expect(result).toBe('result');
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it('第二次调用命中缓存，不再调用 fetcher', async () => {
    const fetcher = vi.fn().mockResolvedValue('result');

    await withCache('test-key', fetcher);
    const result2 = await withCache('test-key', fetcher);

    expect(result2).toBe('result');
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it('缓存过期后重新调用 fetcher', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce('old').mockResolvedValueOnce('new');

    await withCache('test-key', fetcher, 1000);
    vi.advanceTimersByTime(1001);
    const result = await withCache('test-key', fetcher, 1000);

    expect(result).toBe('new');
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
});

describe('CacheKeys', () => {
  it('包含所有预定义的键前缀', () => {
    expect(CacheKeys.VERSIONS).toBe('versions');
    expect(CacheKeys.JAVA).toBe('java');
    expect(CacheKeys.LOADERS).toBe('loaders');
    expect(CacheKeys.CONFIG).toBe('config');
  });
});
