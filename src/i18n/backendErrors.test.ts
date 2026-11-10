import { describe, expect, it } from 'vitest';
import zhCN from './locales/zh-CN';
import enUS from './locales/en-US';

/**
 * 后端错误码与词条的对应关系
 *
 * Rust 里每新增一条 `LauncherError::localized("code", ..)`，两种语言都必须有
 * `errors.backend.code`，否则英文界面会退化成原文（中文）。
 */
const rustSources = import.meta.glob('/src-tauri/src/**/*.rs', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** 粗粒度包装错误：由 LauncherError 的各变体固定产出，不经过 localized() */
const WRAPPER_CODES = new Set([
  'io',
  'http',
  'json',
  'zip',
  'tauri',
  'cancelled',
  'busy',
  'auth_denied',
  'auth_expired',
  'custom',
]);

function codesUsedInRust(): Set<string> {
  const codes = new Set<string>();
  for (const source of Object.values(rustSources)) {
    // 只统计生产代码：测试里的用法和真实错误码无关
    const production = source.split('#[cfg(test)]')[0];
    for (const match of production.matchAll(/LauncherError::localized\(\s*"([a-z0-9_]+)"/g)) {
      codes.add(match[1]);
    }
  }
  return codes;
}

function backendKeys(catalog: unknown): Set<string> {
  const backend = (catalog as { errors?: { backend?: Record<string, string> } }).errors?.backend;
  return new Set(Object.keys(backend ?? {}));
}

const rustCodes = codesUsedInRust();
const zhKeys = backendKeys(zhCN);
const enKeys = backendKeys(enUS);

describe('后端错误码文案', () => {
  it('Rust 里确实声明了细分错误码', () => {
    expect(rustCodes.size).toBeGreaterThan(3);
  });

  it('每个 Rust 错误码在两种语言里都有词条', () => {
    const missingZh = [...rustCodes].filter((code) => !zhKeys.has(code));
    const missingEn = [...rustCodes].filter((code) => !enKeys.has(code));

    expect(missingZh).toEqual([]);
    expect(missingEn).toEqual([]);
  });

  it('没有只在一种语言里存在的词条', () => {
    expect([...zhKeys].filter((key) => !enKeys.has(key))).toEqual([]);
    expect([...enKeys].filter((key) => !zhKeys.has(key))).toEqual([]);
  });

  it('除粗粒度包装外，每个词条都能在 Rust 里找到来源', () => {
    const orphaned = [...zhKeys].filter((key) => !WRAPPER_CODES.has(key) && !rustCodes.has(key));
    expect(orphaned).toEqual([]);
  });
});
