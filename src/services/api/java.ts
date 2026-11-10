/** Java 探测、自定义路径与托管运行时下载 */
import { CacheKeys, cache, withCache } from '../cache';
import { invokeCommand } from '../invoke';
import type { JavaCompatibilityResult, JavaDownloadOption, JavaDownloadProgress } from '../../types/events';
import { dedupedInvoke, dedupedInvokeLong, invokeLong } from './internal';

export const javaApi = {
  async findJavaInstallations(): Promise<string[]> {
    return withCache(
      CacheKeys.JAVA,
      () => dedupedInvokeLong<string[]>('find_java_installations_command'),
      10 * 60 * 1000,
    );
  },

  async refreshJavaInstallations(): Promise<string[]> {
    const result = await invokeLong<string[]>('refresh_java_installations');
    // 必须等后端刷完再失效，否则并发的 findJavaInstallations 会把旧列表写回缓存
    cache.delete(CacheKeys.JAVA);
    return result;
  },

  async setJavaPath(path: string): Promise<void> {
    return invokeCommand('set_java_path_command', { path });
  },

  async validateJavaPath(path: string): Promise<boolean> {
    return dedupedInvoke<boolean>('validate_java_path', { path });
  },

  async getJavaVersion(path: string): Promise<string> {
    return dedupedInvoke<string>('get_java_version', { path });
  },

  async getCustomJavaPaths(): Promise<string[]> {
    return dedupedInvoke<string[]>('get_custom_java_paths');
  },

  async addCustomJavaPath(path: string): Promise<void> {
    return invokeCommand('add_custom_java_path', { path });
  },

  async removeCustomJavaPath(path: string): Promise<void> {
    return invokeCommand('remove_custom_java_path', { path });
  },

  async checkJavaCompatibility(mcVersion: string): Promise<JavaCompatibilityResult> {
    return invokeLong<JavaCompatibilityResult>('check_java_compatibility', { mcVersion });
  },

  async getJavaDownloadOptions(): Promise<JavaDownloadOption[]> {
    return invokeCommand<JavaDownloadOption[]>('get_java_download_options');
  },

  async getJavaDownloadState(): Promise<JavaDownloadProgress | null> {
    return invokeCommand<JavaDownloadProgress | null>('get_java_download_state');
  },

  async downloadJavaRuntime(feature: number): Promise<void> {
    return invokeLong('download_java_runtime', { feature });
  },

  async cancelJavaDownload(): Promise<void> {
    return invokeCommand('cancel_java_download');
  },

  async deleteJavaRuntime(feature: number): Promise<void> {
    return invokeCommand('delete_java_runtime', { feature });
  },
};
