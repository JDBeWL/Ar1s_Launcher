import { describe, expect, it } from 'vitest';
import {
  readCargoVersion,
  readReadmeBadge,
  rewriteCargoVersion,
  rewriteReadmeBadge,
  rewriteTauriVersion,
} from './sync-version.mjs';

/** 真实的 Cargo.toml 片段：依赖里也有 version，还有 rust-version 这种前缀相似的键 */
const CARGO = `[package]
name = "ar1s_launcher"
version = "0.8.5"
description = "Ar1s Launcher"
rust-version = "1.95.0"
license = "MIT"

[build-dependencies]
tauri-build = { version = "2.7", features = [] }

[dependencies]
uuid = { version = "1.28", features = ["v3"] }
serde = { version = "1.0", features = ["derive"] }
`;

describe('Cargo.toml 版本号', () => {
  it('只读 [package] 段的 version，不误取依赖版本', () => {
    expect(readCargoVersion(CARGO)).toBe('0.8.5');
  });

  it('不把 rust-version 当成版本号', () => {
    // rust-version 出现在 version 之前，前缀匹配的实现会在这里出错
    const text = `[package]\nrust-version = "1.95.0"\nversion = "1.2.3"\n`;
    expect(readCargoVersion(text)).toBe('1.2.3');
  });

  it('改写只动 [package] 段，依赖与 rust-version 原样保留', () => {
    const out = rewriteCargoVersion(CARGO, '9.9.9');

    expect(readCargoVersion(out)).toBe('9.9.9');
    expect(out).toContain('rust-version = "1.95.0"');
    expect(out).toContain('tauri-build = { version = "2.7", features = [] }');
    expect(out).toContain('uuid = { version = "1.28", features = ["v3"] }');
    expect(out).toContain('serde = { version = "1.0", features = ["derive"] }');
    // 只有一行 version 被改
    expect(out.match(/9\.9\.9/g)).toHaveLength(1);
  });

  it('没有 [package] 段时返回 null', () => {
    expect(readCargoVersion('[dependencies]\nserde = "1.0"\n')).toBeNull();
  });
});

describe('README badge', () => {
  const README = `# Ar1s Launcher

![Version](https://img.shields.io/badge/version-0.7.2-blue)
![Tauri](https://img.shields.io/badge/Tauri-2.12-orange)
`;

  it('取出 badge 里的版本', () => {
    expect(readReadmeBadge(README)).toBe('0.7.2');
  });

  it('只替换版本 badge，其他 badge 不受影响', () => {
    const out = rewriteReadmeBadge(README, '0.8.5');

    expect(readReadmeBadge(out)).toBe('0.8.5');
    expect(out).toContain('badge/Tauri-2.12-orange');
  });

  it('没有 badge 时返回 null', () => {
    expect(readReadmeBadge('# 没有 badge 的文档\n')).toBeNull();
  });
});

describe('tauri.conf.json 版本号', () => {
  const CONF = `{
  "productName": "Ar1s Launcher",
  "version": "0.8.5",
  "identifier": "com.jdbewl.ar1s-launcher"
}
`;

  it('只替换顶层 version，缩进与键顺序保持不变', () => {
    const out = rewriteTauriVersion(CONF, '1.0.0');

    expect(JSON.parse(out).version).toBe('1.0.0');
    expect(JSON.parse(out).productName).toBe('Ar1s Launcher');
    // 不能因为 JSON 往返把格式重排
    expect(out).toBe(CONF.replace('0.8.5', '1.0.0'));
  });
});
