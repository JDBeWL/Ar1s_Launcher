/**
 * API 层公共设施
 *
 * 并发去重、缓存失效、长任务超时声明都收在这里，各分域文件只描述「调哪个命令、传什么参数」，
 * 避免每个域各写一套去重逻辑
 */
import { CacheKeys, cache } from '../cache';
import { NO_TIMEOUT, invokeCommand, type InvokeCommandOptions } from '../invoke';

const pendingRequests = new Map<string, Promise<unknown>>();

function invalidateGameDirInfo(): void {
  cache.deleteByPrefix(`${CacheKeys.CONFIG}:game_dir_info`);
}

/** 安装/下载会改动 versions 目录，前后都失效，避免事件监听读到旧缓存 */
export async function withGameDirInfoInvalidation<T>(operation: () => Promise<T>): Promise<T> {
  invalidateGameDirInfo();
  try {
    return await operation();
  } finally {
    invalidateGameDirInfo();
  }
}

/** 相同命令 + 相同参数同时只发一次；写操作靠缓存失效保证新鲜度，不走这里 */
export async function dedupedInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  options?: InvokeCommandOptions,
): Promise<T> {
  const key = `${cmd}:${JSON.stringify(args ?? {})}`;
  const pending = pendingRequests.get(key) as Promise<T> | undefined;
  if (pending) return pending;

  const promise = invokeCommand<T>(cmd, args, options).finally(() => {
    pendingRequests.delete(key);
  });

  pendingRequests.set(key, promise);
  return promise;
}

/**
 * 长任务：下载、安装、解压、扫描磁盘、等浏览器回调
 *
 * 不设超时是调用点的显式决定，不再依赖「命令名白名单」——白名单漏一个命令，
 * 用户就会在两分钟后撞上莫名其妙的超时，而调用点完全看不出来
 */
export function invokeLong<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invokeCommand<T>(cmd, args, NO_TIMEOUT);
}

/** 长任务且值得去重：读大目录、列存档这类可能被多个面板同时要的命令 */
export function dedupedInvokeLong<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return dedupedInvoke<T>(cmd, args, NO_TIMEOUT);
}
