#!/usr/bin/env node
/**
 * 版本号单一来源工具
 *
 * 版本号写在三个地方，缺一不可：
 *   - package.json              前端与 npm 生态读它
 *   - src-tauri/Cargo.toml      Rust 编译期 env!("CARGO_PKG_VERSION") 用它（--versionType、诊断包）
 *   - src-tauri/tauri.conf.json Tauri 打包与更新检查用它
 *
 * 手工改三处必然漂移（README 的 badge 曾停在 0.7.2 没人发现）。这里以 package.json 为准，
 * 其余两处与 README badge 都由它派生。
 *
 * 用法：
 *   node scripts/sync-version.mjs           # 检查，不一致就退出码 1（CI 用）
 *   node scripts/sync-version.mjs --write   # 把 package.json 的版本同步到其余位置
 */

import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const PACKAGE_JSON = path.join(root, 'package.json');
const CARGO_TOML = path.join(root, 'src-tauri', 'Cargo.toml');
const TAURI_CONF = path.join(root, 'src-tauri', 'tauri.conf.json');
const READMES = [path.join(root, 'README.md'), path.join(root, 'README.zh-CN.md')];

/**
 * Cargo.toml 里只认 [package] 段下的 version
 *
 * 不能取「第一个 version =」：依赖里也有 `version = "2.7"`，
 * 而且 build-dependencies / target 段都可能出现。按段落扫，只认 [package] 之内
 */
export function readCargoVersion(text) {
  let inPackage = false;
  for (const line of text.split(/\r?\n/)) {
    const section = line.match(/^\s*\[([^\]]+)\]/);
    if (section) {
      inPackage = section[1].trim() === 'package';
      continue;
    }
    if (!inPackage) continue;
    const match = line.match(/^\s*version\s*=\s*"([^"]+)"/);
    if (match) return match[1];
  }
  return null;
}

export function rewriteCargoVersion(text, version) {
  let inPackage = false;
  return text
    .split(/\r?\n/)
    .map((line) => {
      const section = line.match(/^\s*\[([^\]]+)\]/);
      if (section) {
        inPackage = section[1].trim() === 'package';
        return line;
      }
      if (inPackage && /^\s*version\s*=\s*"/.test(line)) {
        return line.replace(/^(\s*version\s*=\s*")[^"]+(")/, `$1${version}$2`);
      }
      return line;
    })
    .join('\n');
}

/** README 的 badge：![Version](https://img.shields.io/badge/version-0.8.5-blue) */
export function readReadmeBadge(text) {
  const match = text.match(/img\.shields\.io\/badge\/version-([0-9A-Za-z.\-+]+)-/);
  return match ? match[1] : null;
}

export function rewriteReadmeBadge(text, version) {
  return text.replace(/(img\.shields\.io\/badge\/version-)[0-9A-Za-z.\-+]+(-)/, `$1${version}$2`);
}

/**
 * 只替换 tauri.conf.json 里第一个 "version" 的值
 *
 * 用 JSON.parse + stringify 往返会重排键顺序、丢掉原有缩进，diff 会变得没法看
 */
export function rewriteTauriVersion(text, version) {
  return text.replace(/("version"\s*:\s*")[^"]*(")/, `$1${version}$2`);
}

function main() {
  const write = process.argv.includes('--write');

  const pkg = JSON.parse(readFileSync(PACKAGE_JSON, 'utf8'));
  const expected = pkg.version;
  if (typeof expected !== 'string' || !/^\d+\.\d+\.\d+/.test(expected)) {
    console.error(`✗ package.json 的 version 不是合法语义化版本：${JSON.stringify(expected)}`);
    process.exit(1);
  }

  const problems = [];

  // Cargo.toml
  const cargoText = readFileSync(CARGO_TOML, 'utf8');
  const cargoVersion = readCargoVersion(cargoText);
  if (cargoVersion !== expected) {
    problems.push(`src-tauri/Cargo.toml 是 ${cargoVersion ?? '(未找到)'}，应为 ${expected}`);
    if (write) writeFileSync(CARGO_TOML, rewriteCargoVersion(cargoText, expected));
  }

  // tauri.conf.json
  const tauriText = readFileSync(TAURI_CONF, 'utf8');
  const tauriVersion = JSON.parse(tauriText).version;
  if (tauriVersion !== expected) {
    problems.push(`src-tauri/tauri.conf.json 是 ${tauriVersion ?? '(未找到)'}，应为 ${expected}`);
    if (write) writeFileSync(TAURI_CONF, rewriteTauriVersion(tauriText, expected));
  }

  // README badge
  for (const readme of READMES) {
    let text;
    try {
      text = readFileSync(readme, 'utf8');
    } catch {
      continue;
    }
    const badge = readReadmeBadge(text);
    if (badge !== expected) {
      problems.push(`${path.basename(readme)} 的版本 badge 是 ${badge ?? '(未找到)'}，应为 ${expected}`);
      if (write) writeFileSync(readme, rewriteReadmeBadge(text, expected));
    }
  }

  if (problems.length === 0) {
    console.log(`✓ 版本号一致：${expected}`);
    return;
  }

  for (const problem of problems) {
    console.error(`✗ ${problem}`);
  }

  if (write) {
    console.log(`\n已把以上位置统一为 ${expected}，请检查 diff 后提交。`);
    return;
  }

  console.error('\n运行 `pnpm version:sync` 让它们与 package.json 一致，或手动修改。');
  process.exit(1);
}

// 被测试 import 时不要执行主流程
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
