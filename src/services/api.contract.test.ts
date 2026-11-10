import { describe, expect, it } from 'vitest';

const frontendTsSources = import.meta.glob('/src/**/*.ts', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as unknown as Record<string, string>;

const frontendVueSources = import.meta.glob('/src/**/*.vue', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as unknown as Record<string, string>;

const rustSources = import.meta.glob('/src-tauri/src/**/*.rs', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as unknown as Record<string, string>;

interface RustParam {
  name: string;
  optional: boolean;
}

interface RustCommand {
  name: string;
  params: RustParam[];
}

interface TsInvocation {
  command: string;
  keys: Set<string>;
  file: string;
}

function toCamelCase(name: string): string {
  return name.replace(/_([a-z0-9])/g, (_match, char: string) => char.toUpperCase());
}

function findMatching(source: string, openIndex: number, open: string, close: string): number {
  let depth = 0;
  let quote: string | null = null;

  for (let i = openIndex; i < source.length; i += 1) {
    const char = source.charAt(i);
    if (quote) {
      if (char === '\\') i += 1;
      else if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'" || char === '`') {
      quote = char;
      continue;
    }
    if (char === open) depth += 1;
    else if (char === close) {
      depth -= 1;
      if (depth === 0) return i;
    }
  }

  throw new Error(`找不到配对的 ${open}${close}`);
}

function splitTopLevel(input: string, separator: string, trackAngles: boolean): string[] {
  const parts: string[] = [];
  const closing: Record<string, string> = { '(': ')', '[': ']', '{': '}', '<': '>' };
  const stack: string[] = [];
  let quote: string | null = null;
  let start = 0;

  for (let i = 0; i < input.length; i += 1) {
    const char = input.charAt(i);
    if (quote) {
      if (char === '\\') i += 1;
      else if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'" || char === '`') {
      quote = char;
      continue;
    }
    if (char === '(' || char === '[' || char === '{' || (trackAngles && char === '<')) {
      stack.push(closing[char]);
      continue;
    }
    if (char === ')' || char === ']' || char === '}' || (trackAngles && char === '>')) {
      if (char === '>' && input.charAt(i - 1) === '-') continue;
      if (stack[stack.length - 1] === char) stack.pop();
      continue;
    }
    if (char === separator && stack.length === 0) {
      parts.push(input.slice(start, i));
      start = i + 1;
    }
  }

  parts.push(input.slice(start));
  return parts;
}

function findTopLevelColon(input: string): number {
  let depth = 0;
  let quote: string | null = null;

  for (let i = 0; i < input.length; i += 1) {
    const char = input.charAt(i);
    if (quote) {
      if (char === '\\') i += 1;
      else if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'" || char === '`') {
      quote = char;
      continue;
    }
    if (char === '(' || char === '[' || char === '{') depth += 1;
    else if (char === ')' || char === ']' || char === '}') depth -= 1;
    else if (char === ':' && depth === 0) return i;
  }

  return -1;
}

function extractObjectKeys(inner: string, file: string, command: string): Set<string> {
  const keys = new Set<string>();

  for (const part of splitTopLevel(inner, ',', false)) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    if (trimmed.startsWith('...')) {
      throw new Error(`${file}: 命令 ${command} 的参数不能使用展开语法`);
    }

    const colon = findTopLevelColon(trimmed);
    const rawKey = (colon === -1 ? trimmed : trimmed.slice(0, colon)).trim();
    const key = rawKey.replace(/^['"]/, '').replace(/['"]$/, '');

    if (!/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key)) {
      throw new Error(`${file}: 无法解析命令 ${command} 的参数键 ${rawKey}`);
    }

    keys.add(key);
  }

  return keys;
}

function parseRustCommands(sources: Record<string, string>): Map<string, RustCommand> {
  const commands = new Map<string, RustCommand>();

  for (const source of Object.values(sources)) {
    for (const marker of source.matchAll(/#\[tauri::command\]/g)) {
      const rest = source.slice((marker.index ?? 0) + marker[0].length);
      const signature = /\bpub\s+(?:async\s+)?fn\s+([a-z0-9_]+)\s*\(/.exec(rest);
      if (!signature) continue;

      const name = signature[1];
      const openIndex = (signature.index ?? 0) + signature[0].length - 1;
      const closeIndex = findMatching(rest, openIndex, '(', ')');
      const rawParams = rest.slice(openIndex + 1, closeIndex);
      const params: RustParam[] = [];

      for (const rawParam of splitTopLevel(rawParams, ',', true)) {
        const param = rawParam.trim();
        if (!param) continue;

        const colon = param.indexOf(':');
        if (colon === -1) continue;

        const paramName = param
          .slice(0, colon)
          .trim()
          .replace(/^mut\s+/, '');
        const paramType = param.slice(colon + 1).trim();

        // window/app 等由 Tauri 注入，前端不会传
        if (['window', 'app', 'app_handle', 'state', 'webview'].includes(paramName)) continue;
        if (paramType.includes('tauri::') || paramType === 'Window' || paramType === 'AppHandle') continue;

        params.push({
          name: toCamelCase(paramName),
          optional: paramType.startsWith('Option<'),
        });
      }

      commands.set(name, { name, params });
    }
  }

  return commands;
}

function parseTsInvocations(sources: Record<string, string>): TsInvocation[] {
  const invocations: TsInvocation[] = [];

  for (const [file, source] of Object.entries(sources)) {
    if (/\.(test|spec)\.ts$/.test(file)) continue;

    for (const match of source.matchAll(/\b(invokeCommand|dedupedInvokeLong|dedupedInvoke|invokeLong|invoke)\b/g)) {
      const start = match.index ?? 0;
      if (source.charAt(start - 1) === '.') continue;

      let cursor = start + match[0].length;
      while (/\s/.test(source.charAt(cursor))) cursor += 1;

      if (source.charAt(cursor) === '<') {
        cursor = findMatching(source, cursor, '<', '>') + 1;
        while (/\s/.test(source.charAt(cursor))) cursor += 1;
      }

      if (source.charAt(cursor) !== '(') continue;

      const closeIndex = findMatching(source, cursor, '(', ')');
      const args = splitTopLevel(source.slice(cursor + 1, closeIndex), ',', false).map((part) => part.trim());
      const commandLiteral = /^['"]([a-z0-9_]+)['"]$/.exec(args[0] ?? '');
      if (!commandLiteral) continue;

      const command = commandLiteral[1];
      const secondArg = args[1];
      let keys = new Set<string>();

      if (secondArg) {
        if (!secondArg.startsWith('{')) {
          throw new Error(`${file}: 命令 ${command} 的参数必须写成内联对象，契约测试才能校验`);
        }
        const objectClose = findMatching(secondArg, 0, '{', '}');
        if (objectClose !== secondArg.length - 1) {
          throw new Error(`${file}: 命令 ${command} 的参数后面还有多余内容`);
        }
        keys = extractObjectKeys(secondArg.slice(1, objectClose), file, command);
      }

      invocations.push({ command, keys, file });
    }
  }

  return invocations;
}

const rustCommands = parseRustCommands(rustSources);
const invocations = parseTsInvocations({ ...frontendTsSources, ...frontendVueSources });
const registeredCommands = new Set<string>();
const libSource = Object.entries(rustSources).find(([file]) => file.endsWith('/lib.rs'))?.[1] ?? '';

for (const match of libSource.matchAll(/controllers::[a-z_]+::([a-z0-9_]+)/g)) {
  registeredCommands.add(match[1]);
}

describe('Tauri 命令契约', () => {
  it('前端调用的命令与 generate_handler 注册列表一致', () => {
    const usedCommands = new Set(invocations.map((invocation) => invocation.command));

    expect([...usedCommands].sort()).toEqual([...registeredCommands].sort());
  });

  it('控制器里的命令签名与注册列表一致', () => {
    expect([...rustCommands.keys()].sort()).toEqual([...registeredCommands].sort());
  });

  it('invoke 参数名与 Rust 命令签名一致', () => {
    const failures: string[] = [];

    for (const invocation of invocations) {
      const command = rustCommands.get(invocation.command);
      if (!command) {
        failures.push(`${invocation.file}: 后端没有命令 ${invocation.command}`);
        continue;
      }

      const rustKeys = new Set(command.params.map((param) => param.name));
      for (const key of invocation.keys) {
        if (!rustKeys.has(key)) {
          failures.push(`${invocation.command}: 前端多传参数 ${key}（Rust 参数: ${[...rustKeys].join(', ') || '无'}）`);
        }
      }

      for (const param of command.params) {
        if (!param.optional && !invocation.keys.has(param.name)) {
          failures.push(`${invocation.command}: 缺少必填参数 ${param.name}`);
        }
      }
    }

    expect(failures).toEqual([]);
  });
});
