/**
 * Tauri 命令调用层
 * 统一错误形状与超时：后端拒绝的 { code, message } 转成 LauncherCommandError，卡死的命令不会让界面无限等待
 */
import { invoke } from '@tauri-apps/api/core';
import { translate } from '../i18n';
import { logWarn } from '../utils/logger';

export const DEFAULT_COMMAND_TIMEOUT_MS = 120_000;

/** 下载、安装、扫描磁盘等天然可能超过两分钟的命令不设超时 */
const UNBOUNDED_TIMEOUT_COMMANDS = new Set([
  'download_version',
  'create_instance',
  'install_modrinth_modpack',
  'install_local_modpack',
  'install_modrinth_mod',
  'install_curseforge_modpack',
  'install_curseforge_mod',
  'preview_local_modpack',
  'export_modpack',
  'validate_version_files',
  'get_game_dir_info',
  'get_instances',
  'get_instance_mods',
  'get_instance_saves',
  'get_save_backups',
  'backup_save',
  'restore_save_backup',
  'delete_save_backup',
  'import_saves',
  'get_resource_packs',
  'import_resource_packs',
  'install_modrinth_resource_pack',
  'get_shader_packs',
  'import_shader_packs',
  'install_modrinth_shader_pack',
  'find_java_installations_command',
  'refresh_java_installations',
  'check_java_compatibility',
  'export_diagnostic_bundle',
  'launch_instance',
  'complete_microsoft_login',
  'complete_microsoft_auth_code_login',
]);

export interface InvokeCommandOptions {
  /** 0 表示本次调用不设超时 */
  timeoutMs?: number;
}

export class LauncherCommandError extends Error {
  readonly command: string;
  readonly code: string;
  readonly raw: unknown;

  constructor(command: string, code: string, message: string, raw: unknown) {
    super(message);
    this.name = 'LauncherCommandError';
    this.command = command;
    this.code = code;
    this.raw = raw;
  }

  get cancelled(): boolean {
    return this.code === 'cancelled';
  }
}

function extractCode(error: unknown): string | null {
  if (!error || typeof error !== 'object' || !('code' in error)) return null;
  const code = (error as { code: unknown }).code;
  if (code == null) return null;
  return typeof code === 'string' ? code : String(code);
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
  return new LauncherCommandError(command, extractCode(error) ?? 'ipc', extractMessage(error), error);
}

export function commandTimeoutMs(command: string, override?: number): number {
  if (override !== undefined) return Math.max(0, override);
  return UNBOUNDED_TIMEOUT_COMMANDS.has(command) ? 0 : DEFAULT_COMMAND_TIMEOUT_MS;
}

export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
  options: InvokeCommandOptions = {},
): Promise<T> {
  const timeoutMs = commandTimeoutMs(command, options.timeoutMs);
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
