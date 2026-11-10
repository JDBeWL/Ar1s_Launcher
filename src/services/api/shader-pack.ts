/** 光影包：本地目录管理 + Modrinth 搜索安装 */
import { invokeCommand } from '../invoke';
import type {
  InstalledShaderPack,
  ModrinthSearchResult,
  ModrinthVersion,
  ShaderPackImportSummary,
  ShaderPackList,
  ShaderPackUpdate,
} from '../../types/events';
import { dedupedInvoke, invokeLong } from './internal';

export const shaderPackApi = {
  /** 实例 shaderpacks 目录里的光影包，含配置里的选中状态；状态会变，不走缓存 */
  async getShaderPacks(instanceName: string): Promise<ShaderPackList> {
    return invokeLong<ShaderPackList>('get_shader_packs', { instanceName });
  },

  /** 选中并启用一个光影包；写回 iris.properties / oculus.properties / optionsshaders.txt */
  async selectShaderPack(instanceName: string, fileName: string): Promise<ShaderPackList> {
    return invokeCommand<ShaderPackList>('set_shader_pack', { instanceName, fileName });
  },

  /** 关闭光影：Iris 只关总开关，OptiFine 写 shaderPack=OFF */
  async disableShaderPack(instanceName: string): Promise<ShaderPackList> {
    return invokeCommand<ShaderPackList>('set_shader_pack', { instanceName, fileName: null });
  },

  /** Iris / Oculus 的总开关，OptiFine 不支持单独打开 */
  async setShadersEnabled(instanceName: string, enabled: boolean): Promise<ShaderPackList> {
    return invokeCommand<ShaderPackList>('set_shaders_enabled', { instanceName, enabled });
  },

  async deleteShaderPack(instanceName: string, fileName: string): Promise<void> {
    await invokeCommand('delete_instance_shader_pack', { instanceName, fileName });
  },

  /** 把本地选中的 zip 或文件夹复制进实例的 shaderpacks */
  async importShaderPacks(instanceName: string, sourcePaths: string[]): Promise<ShaderPackImportSummary> {
    return invokeLong<ShaderPackImportSummary>('import_shader_packs', { instanceName, sourcePaths });
  },

  async openFolder(instanceName: string): Promise<void> {
    await invokeCommand('open_shaderpacks_folder', { instanceName });
  },

  /** 只按实例自身的 MC 版本搜索；光影包在 Modrinth 上挂 iris / optifine 两个 loader */
  async searchModrinthShaderPacks(
    instanceName: string,
    params: { query?: string; limit?: number; offset?: number; sortBy?: string },
  ): Promise<ModrinthSearchResult> {
    return dedupedInvoke<ModrinthSearchResult>('search_modrinth_shader_packs', {
      instanceName,
      query: params.query,
      limit: params.limit,
      offset: params.offset,
      sortBy: params.sortBy,
    });
  },

  async getModrinthShaderPackVersions(instanceName: string, projectId: string): Promise<ModrinthVersion[]> {
    return dedupedInvoke<ModrinthVersion[]>('get_modrinth_shader_pack_versions', {
      instanceName,
      projectId,
    });
  },

  async installModrinthShaderPack(
    instanceName: string,
    projectId: string,
    versionId?: string,
  ): Promise<InstalledShaderPack> {
    return invokeLong<InstalledShaderPack>('install_modrinth_shader_pack', {
      instanceName,
      projectId,
      versionId,
    });
  },

  /** 用本地 zip 的 sha1 反查更新，已是最新的文件不会返回 */
  async checkUpdates(instanceName: string): Promise<ShaderPackUpdate[]> {
    return dedupedInvoke<ShaderPackUpdate[]>('check_modrinth_shader_pack_updates', { instanceName });
  },

  async updateShaderPack(instanceName: string, fileName: string, versionId: string): Promise<InstalledShaderPack> {
    return invokeLong<InstalledShaderPack>('update_modrinth_shader_pack', {
      instanceName,
      fileName,
      versionId,
    });
  },
};
