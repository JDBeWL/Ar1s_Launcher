/**
 * CurseForge 数据源（麦块开放 API 中转，响应与 Modrinth 命令同构）
 */
import { CacheKeys, withCache } from '../cache';
import type {
  CfCategory,
  CfManualFile,
  CfProjectRef,
  ModInstallResult,
  ModrinthSearchResult,
  ModrinthVersion,
} from '../../types/events';
import { dedupedInvoke, invokeLong, withGameDirInfoInvalidation } from './internal';

export const curseforgeApi = {
  /** 只按实例自身的 MC 版本与加载器搜索 */
  async searchMods(
    instanceName: string,
    params: { query?: string; limit?: number; offset?: number; sortBy?: string },
  ): Promise<ModrinthSearchResult> {
    return dedupedInvoke<ModrinthSearchResult>('search_curseforge_mods', {
      instanceName,
      query: params.query,
      limit: params.limit,
      offset: params.offset,
      sortBy: params.sortBy,
    });
  },

  async getModVersions(instanceName: string, projectId: string): Promise<ModrinthVersion[]> {
    return dedupedInvoke<ModrinthVersion[]>('get_curseforge_mod_versions', {
      instanceName,
      projectId,
    });
  },

  /** fileId 缺省时按实例版本挑最新文件；CF 文件不带依赖信息，不补依赖 */
  async installMod(instanceName: string, projectId: string, fileId?: string): Promise<ModInstallResult> {
    return invokeLong<ModInstallResult>('install_curseforge_mod', {
      instanceName,
      projectId,
      fileId,
    });
  },

  async searchModpacks(params: {
    query?: string;
    gameVersions?: string[];
    loaders?: string[];
    categories?: string[];
    limit?: number;
    offset?: number;
    sortBy?: string;
  }): Promise<ModrinthSearchResult> {
    return dedupedInvoke<ModrinthSearchResult>('search_curseforge_modpacks', {
      query: params.query,
      gameVersions: params.gameVersions,
      loaders: params.loaders,
      categories: params.categories,
      limit: params.limit,
      offset: params.offset,
      sortBy: params.sortBy,
    });
  },

  async getModpackVersions(projectId: string): Promise<ModrinthVersion[]> {
    return dedupedInvoke<ModrinthVersion[]>('get_curseforge_modpack_versions', { projectId });
  },

  /** CF 整合包的分类标签；slug 与 Modrinth 的 taxonomy 不通用 */
  async getModpackCategories(): Promise<CfCategory[]> {
    return withCache(
      `${CacheKeys.CONFIG}:cf_modpack_categories`,
      () => dedupedInvoke<CfCategory[]>('get_curseforge_modpack_categories'),
      10 * 60 * 1000,
    );
  },

  /** 把链接/slug/数字 ID 解析成项目引用；搜索库未收录的新包也能装 */
  async resolveModpackRef(input: string): Promise<CfProjectRef> {
    return dedupedInvoke<CfProjectRef>('resolve_curseforge_modpack', { input });
  },

  /** 返回需手动下载的受限文件清单，安装本体成功时为空数组 */
  async installModpack(options: {
    modpack_id: string;
    version_id: string;
    instance_name: string;
    install_path: string;
  }): Promise<CfManualFile[]> {
    return withGameDirInfoInvalidation(() => invokeLong<CfManualFile[]>('install_curseforge_modpack', { options }));
  },
};
