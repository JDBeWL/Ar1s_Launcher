#!/usr/bin/env node
/**
 * 版本号单一来源工具
 * 以 package.json 为准，把 Cargo.toml、tauri.conf.json 与 README badge 同步成它；不带 --write 时只检查。
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
 * Cargo.toml 里只认 [package] 段下的 version：依赖段和 target 段里也有 `version = "…"`，
 * 取第一个会读到依赖的版本号
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
 * 只替换第一个 "version" 的值：JSON.parse + stringify 往返会重排键顺序、丢掉缩进，diff 会变得没法看
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

  const cargoText = readFileSync(CARGO_TOML, 'utf8');
  const cargoVersion = readCargoVersion(cargoText);
  if (cargoVersion !== expected) {
    problems.push(`src-tauri/Cargo.toml 是 ${cargoVersion ?? '(未找到)'}，应为 ${expected}`);
    if (write) writeFileSync(CARGO_TOML, rewriteCargoVersion(cargoText, expected));
  }

  const tauriText = readFileSync(TAURI_CONF, 'utf8');
  const tauriVersion = JSON.parse(tauriText).version;
  if (tauriVersion !== expected) {
    problems.push(`src-tauri/tauri.conf.json 是 ${tauriVersion ?? '(未找到)'}，应为 ${expected}`);
    if (write) writeFileSync(TAURI_CONF, rewriteTauriVersion(tauriText, expected));
  }

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
