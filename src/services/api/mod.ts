/** 模组：Modrinth 搜索、安装与更新检查 */
import type { ModInstallResult, ModUpdate, ModrinthSearchResult, ModrinthVersion } from '../../types/events';
import { dedupedInvoke, invokeLong } from './internal';

export const modApi = {
  /** 只按实例自身的 MC 版本与加载器搜索，避免装上不兼容的模组 */
  async searchModrinthMods(
    instanceName: string,
    params: { query?: string; limit?: number; offset?: number; sortBy?: string },
  ): Promise<ModrinthSearchResult> {
    return dedupedInvoke<ModrinthSearchResult>('search_modrinth_mods', {
      instanceName,
      query: params.query,
      limit: params.limit,
      offset: params.offset,
      sortBy: params.sortBy,
    });
  },

  async getModrinthModVersions(instanceName: string, projectId: string): Promise<ModrinthVersion[]> {
    return dedupedInvoke<ModrinthVersion[]>('get_modrinth_mod_versions', {
      instanceName,
      projectId,
    });
  },

  async installModrinthMod(
    instanceName: string,
    projectId: string,
    versionId?: string,
    installDependencies = true,
  ): Promise<ModInstallResult> {
    return invokeLong<ModInstallResult>('install_modrinth_mod', {
      instanceName,
      projectId,
      versionId,
      installDependencies,
    });
  },

  /** 用本地 jar 的 sha1 反查更新，已是最新的文件不会返回 */
  async checkModUpdates(instanceName: string): Promise<ModUpdate[]> {
    return dedupedInvoke<ModUpdate[]>('check_modrinth_mod_updates', { instanceName });
  },

  async updateMod(instanceName: string, fileName: string, versionId: string): Promise<ModInstallResult> {
    return invokeLong<ModInstallResult>('update_modrinth_mod', {
      instanceName,
      fileName,
      versionId,
    });
  },
};
