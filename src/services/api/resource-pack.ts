/** 资源包：本地目录管理 + Modrinth 搜索安装 */
import { invokeCommand } from '../invoke';
import type {
  InstalledResourcePack,
  ModrinthSearchResult,
  ModrinthVersion,
  ResourcePackImportSummary,
  ResourcePackInfo,
  ResourcePackUpdate,
} from '../../types/events';
import { dedupedInvoke, invokeLong } from './internal';

export const resourcePackApi = {
  /** 实例 resourcepacks 目录里的资源包，含启用状态；状态会变，不走缓存 */
  async getResourcePacks(instanceName: string): Promise<ResourcePackInfo[]> {
    return invokeLong<ResourcePackInfo[]>('get_resource_packs', { instanceName });
  },

  /** 启用/禁用只改 options.txt 的 resourcePacks 列表，不动文件 */
  async setResourcePackEnabled(instanceName: string, fileName: string, enabled: boolean): Promise<void> {
    await invokeCommand('set_resource_pack_enabled', { instanceName, fileName, enabled });
  },

  async deleteResourcePack(instanceName: string, fileName: string): Promise<void> {
    await invokeCommand('delete_instance_resource_pack', { instanceName, fileName });
  },

  /** 把本地选中的 zip 或文件夹复制进实例的 resourcepacks */
  async importResourcePacks(instanceName: string, sourcePaths: string[]): Promise<ResourcePackImportSummary> {
    return invokeLong<ResourcePackImportSummary>('import_resource_packs', { instanceName, sourcePaths });
  },

  async openFolder(instanceName: string): Promise<void> {
    await invokeCommand('open_resourcepacks_folder', { instanceName });
  },

  /** 只按实例自身的 MC 版本搜索，资源包没有加载器维度 */
  async searchModrinthResourcePacks(
    instanceName: string,
    params: { query?: string; limit?: number; offset?: number; sortBy?: string },
  ): Promise<ModrinthSearchResult> {
    return dedupedInvoke<ModrinthSearchResult>('search_modrinth_resource_packs', {
      instanceName,
      query: params.query,
      limit: params.limit,
      offset: params.offset,
      sortBy: params.sortBy,
    });
  },

  async getModrinthResourcePackVersions(instanceName: string, projectId: string): Promise<ModrinthVersion[]> {
    return dedupedInvoke<ModrinthVersion[]>('get_modrinth_resource_pack_versions', {
      instanceName,
      projectId,
    });
  },

  async installModrinthResourcePack(
    instanceName: string,
    projectId: string,
    versionId?: string,
  ): Promise<InstalledResourcePack> {
    return invokeLong<InstalledResourcePack>('install_modrinth_resource_pack', {
      instanceName,
      projectId,
      versionId,
    });
  },

  /** 用本地 zip 的 sha1 反查更新，已是最新的文件不会返回 */
  async checkUpdates(instanceName: string): Promise<ResourcePackUpdate[]> {
    return dedupedInvoke<ResourcePackUpdate[]>('check_modrinth_resource_pack_updates', { instanceName });
  },

  async updateResourcePack(instanceName: string, fileName: string, versionId: string): Promise<InstalledResourcePack> {
    return invokeLong<InstalledResourcePack>('update_modrinth_resource_pack', {
      instanceName,
      fileName,
      versionId,
    });
  },
};
