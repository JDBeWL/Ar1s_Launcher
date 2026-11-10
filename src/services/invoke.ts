/**
 * Tauri 命令调用层
 * 统一错误形状与超时：后端拒绝的 { code, message } 转成 LauncherCommandError，卡死的命令不会让界面无限等待
 */
import { invoke } from '@tauri-apps/api/core';
import { hasTranslation, rawTranslation, translate, translateList } from '../i18n';
import { logWarn } from '../utils/logger';

export const DEFAULT_COMMAND_TIMEOUT_MS = 120_000;

export interface InvokeCommandOptions {
  /** 0 表示本次调用不设超时 */
  timeoutMs?: number;
}

/**
 * 长任务的显式声明
 *
 * 下载、安装、解压、扫描磁盘、等浏览器回调都可能远超默认超时，这些调用在 API 层
 * 用 `invokeLong` / `dedupedInvokeLong` 明确写出来，而不是维护一份「哪些命令例外」的
 * 全局名单：名单漏加一个命令，用户就会撞上莫名其妙的超时，而且从调用点完全看不出来
 */
export const NO_TIMEOUT: InvokeCommandOptions = { timeoutMs: 0 };

export class LauncherCommandError extends Error {
  readonly command: string;
  readonly code: string;
  /** 后端错误码带的位置参数，与错误码文案里的 {0}、{1} 对应 */
  readonly params: string[];
  readonly raw: unknown;

  constructor(command: string, code: string, message: string, raw: unknown, params: string[] = []) {
    super(message);
    this.name = 'LauncherCommandError';
    this.command = command;
    this.code = code;
    this.params = params;
    this.raw = raw;
  }

  get cancelled(): boolean {
    return this.code === 'cancelled';
  }
}

const BACKEND_ERROR_KEY_PREFIX = 'errors.backend.';

function extractCode(error: unknown): string | null {
  if (!error || typeof error !== 'object' || !('code' in error)) return null;
  const code = (error as { code: unknown }).code;
  if (code == null) return null;
  return typeof code === 'string' ? code : String(code);
}

function extractParams(error: unknown): string[] {
  if (!error || typeof error !== 'object' || !('params' in error)) return [];
  const params = (error as { params: unknown }).params;
  if (!Array.isArray(params)) return [];
  return params.map((value) => (value == null ? '' : String(value)));
}

/**
 * 后端错误码 → 当前语言的文案
 *
 * 有专用词条且参数够用就用词条；`custom` 没有细分码，用本地化外壳包住后端原文；
 * 其余情况返回 null，由调用方回退到原文，避免把 `{0}` 之类的占位符露给用户
 */
function localizeBackendError(code: string | null, params: string[], rawMessage: string): string | null {
  if (!code) return null;
  if (code === 'custom') {
    return translateList(`${BACKEND_ERROR_KEY_PREFIX}custom`, [rawMessage]);
  }

  const key = `${BACKEND_ERROR_KEY_PREFIX}${code}`;
  if (!hasTranslation(key)) return null;

  // 新旧版本错配时参数可能不够，此时宁可显示后端原文也不显示半截模板
  const template = rawTranslation(key);
  const placeholderIndexes = template ? [...template.matchAll(/\{(\d+)\}/g)].map((m) => Number(m[1])) : [];
  const required = placeholderIndexes.length > 0 ? Math.max(...placeholderIndexes) + 1 : 0;
  if (params.length < required) return null;

  return translateList(key, params);
}

function extractMessage(error: unknown): string {
  const fallback = translate('errors.globalUnknown');
  if (error instanceof Error) return error.message || fallback;
  if (typeof error === 'string') return error.trim() || fallback;
  if (error && typeof error === 'object') {
    const record = error as Record<string, unknown>;
    if (typeof record.message === 'string' && record.message.trim()) return record.message;
    if (record.message != null) return String(record.message);
    try {
      return JSON.stringify(error);
    } catch {
      return fallback;
    }
  }
  return error == null ? fallback : String(error);
}

export function normalizeCommandError(command: string, error: unknown): LauncherCommandError {
  if (error instanceof LauncherCommandError) return error;

  const code = extractCode(error);
  const params = extractParams(error);
  const rawMessage = extractMessage(error);

  return new LauncherCommandError(
    command,
    code ?? 'ipc',
    localizeBackendError(code, params, rawMessage) ?? rawMessage,
    error,
    params,
  );
}

/** 调用方没声明就用默认值；0 与负数都按「不设超时」处理 */
export function commandTimeoutMs(override?: number): number {
  if (override === undefined) return DEFAULT_COMMAND_TIMEOUT_MS;
  return Math.max(0, override);
}

export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
  options: InvokeCommandOptions = {},
): Promise<T> {
  const timeoutMs = commandTimeoutMs(options.timeoutMs);
  let timer: ReturnType<typeof setTimeout> | undefined;

  try {
    const request = invoke<T>(command, args);
    if (timeoutMs === 0) {
      return await request;
    }

    const timeout = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => {
        reject(
          new LauncherCommandError(
            command,
            'timeout',
            translate('errors.commandTimeout', {
              command,
              seconds: Math.ceil(timeoutMs / 1000),
            }),
            { timeoutMs },
          ),
        );
      }, timeoutMs);
    });

    return await Promise.race([request, timeout]);
  } catch (error) {
    const normalized = normalizeCommandError(command, error);
    if (!normalized.cancelled) {
      logWarn(translate('errors.commandFailed', { command }), normalized, 'Invoke');
    }
    throw normalized;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
