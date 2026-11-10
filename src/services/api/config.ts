/** 全局配置：游戏目录、下载线程、代理、内存与窗口 */
import { CacheKeys, cache, withCache } from '../cache';
import { invokeCommand } from '../invoke';
import type {
  GameConfig,
  GameDirInfo,
  JvmAdvancedOptions,
  MemoryStats,
  ProxySettings,
  ProxyTestResult,
  WindowSettings,
} from '../../types/events';
import { dedupedInvoke, dedupedInvokeLong, withGameDirInfoInvalidation } from './internal';

export const configApi = {
  async getConfig(): Promise<GameConfig> {
    return dedupedInvoke<GameConfig>('get_config');
  },

  async getGameDir(): Promise<string> {
    return dedupedInvoke<string>('get_game_dir');
  },

  async setGameDir(path: string): Promise<void> {
    await withGameDirInfoInvalidation(() => invokeCommand('set_game_dir', { path }));
  },

  async getGameDirInfo(): Promise<GameDirInfo> {
    return withCache(
      `${CacheKeys.CONFIG}:game_dir_info`,
      () => dedupedInvokeLong<GameDirInfo>('get_game_dir_info'),
      2 * 60 * 1000,
    );
  },

  async getDownloadThreads(): Promise<number> {
    return dedupedInvoke<number>('get_download_threads');
  },

  async setDownloadThreads(threads: number): Promise<void> {
    return invokeCommand('set_download_threads', { threads });
  },

  async getProxySettings(): Promise<ProxySettings> {
    return dedupedInvoke<ProxySettings>('get_proxy_settings');
  },

  /** 后端会校验并规范化地址，保存后立即重建共享 HTTP 客户端 */
  async setProxySettings(settings: ProxySettings): Promise<ProxySettings> {
    const applied = await invokeCommand<ProxySettings>('set_proxy_settings', { settings });
    cache.deleteByPrefix(CacheKeys.CONFIG);
    return applied;
  },

  /** 按传入设置临时建客户端测试，不保存配置 */
  async testProxy(settings: ProxySettings): Promise<ProxyTestResult> {
    return invokeCommand<ProxyTestResult>('test_proxy', { settings });
  },

  async loadConfigKey(key: string): Promise<string | null> {
    return dedupedInvoke<string | null>('load_config_key', { key });
  },

  async saveConfigKey(key: string, value: string): Promise<void> {
    await invokeCommand('save_config_key', { key, value });
    cache.deleteByPrefix(CacheKeys.CONFIG);
  },

  async getLastSelectedVersion(): Promise<string | null> {
    return dedupedInvoke<string | null>('get_last_selected_version');
  },

  async setLastSelectedVersion(version: string): Promise<void> {
    return invokeCommand('set_last_selected_version', { version });
  },

  async getMemoryStats(): Promise<MemoryStats> {
    // 占用要实时读，不能缓存；sysinfo 冷启动第一次可能返回 0，补一次
    let stats = await invokeCommand<MemoryStats>('get_memory_stats');
    if (!stats.total_memory_mb) {
      stats = await invokeCommand<MemoryStats>('get_memory_stats');
    }
    return stats;
  },

  async getWindowSettings(): Promise<WindowSettings> {
    return dedupedInvoke<WindowSettings>('get_window_settings');
  },

  async setWindowSettings(width: number | null, height: number | null, fullscreen: boolean): Promise<void> {
    return invokeCommand('set_window_settings', { width, height, fullscreen });
  },

  async getJvmAdvancedOptions(): Promise<JvmAdvancedOptions> {
    return invokeCommand<JvmAdvancedOptions>('get_jvm_advanced_options');
  },

  async setJvmAdvancedOptions(options: JvmAdvancedOptions): Promise<void> {
    return invokeCommand('set_jvm_advanced_options', { options });
  },

  async getEffectiveJvmArgs(memoryMb?: number): Promise<string[]> {
    return invokeCommand<string[]>('get_effective_jvm_args', { memoryMb: memoryMb ?? null });
  },

  async checkMemoryWarning(memoryMb: number): Promise<string | null> {
    return dedupedInvoke<string | null>('check_memory_warning', { memoryMb });
  },
};
