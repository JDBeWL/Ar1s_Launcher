/** 整合包：Modrinth 搜索安装、本地包预览与导出 */
import { invokeCommand } from '../invoke';
import type {
  CfManualFile,
  LocalModpackPreview,
  ModpackExportOptions,
  ModpackExportResult,
  ModrinthSearchResult,
  ModrinthVersion,
} from '../../types/events';
import { dedupedInvoke, invokeLong, withGameDirInfoInvalidation } from './internal';

export const modpackApi = {
  async searchModrinthModpacks(params: {
    query?: string;
    gameVersions?: string[];
    loaders?: string[];
    categories?: string[];
    limit?: number;
    offset?: number;
    sortBy?: string;
  }): Promise<ModrinthSearchResult> {
    return dedupedInvoke<ModrinthSearchResult>('search_modrinth_modpacks', {
      query: params.query,
      gameVersions: params.gameVersions,
      loaders: params.loaders,
      categories: params.categories,
      limit: params.limit,
      offset: params.offset,
      sortBy: params.sortBy,
    });
  },

  async getModrinthModpackVersions(
    projectId: string,
    gameVersions?: string[],
    loaders?: string[],
  ): Promise<ModrinthVersion[]> {
    return dedupedInvoke<ModrinthVersion[]>('get_modrinth_modpack_versions', {
      projectId,
      gameVersions,
      loaders,
    });
  },

  async installModrinthModpack(options: {
    modpack_id: string;
    version_id: string;
    instance_name: string;
    install_path: string;
  }): Promise<void> {
    return withGameDirInfoInvalidation(() => invokeLong('install_modrinth_modpack', { options }));
  },

  /** 读取本地 .mrpack 的清单，安装页据此展示包信息 */
  async previewLocalModpack(filePath: string): Promise<LocalModpackPreview> {
    return invokeLong<LocalModpackPreview>('preview_local_modpack', { filePath });
  },

  /** 从本地压缩包安装（.mrpack 与 CurseForge zip 自动识别），压缩包不会被移动或删除；
   * 返回值是 CurseForge 受限文件的手动下载清单，Modrinth 格式恒为空 */
  async installLocalModpack(options: { filePath: string; instanceName: string }): Promise<CfManualFile[]> {
    return withGameDirInfoInvalidation(() =>
      invokeLong<CfManualFile[]>('install_local_modpack', {
        options: { file_path: options.filePath, instance_name: options.instanceName },
      }),
    );
  },

  /** 把实例打包成 Modrinth 格式的 .mrpack；目标路径由原生保存对话框选定 */
  async exportModpack(instanceName: string, options: ModpackExportOptions): Promise<ModpackExportResult> {
    // options 走 serde camelCase，其余命令参数按 Tauri 的 snake_case->camelCase 约定
    return invokeLong<ModpackExportResult>('export_modpack', { instanceName, options });
  },

  async cancelModpackInstall(): Promise<void> {
    return invokeCommand('cancel_modpack_install');
  },
};
