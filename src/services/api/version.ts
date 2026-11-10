/** 原版版本清单与版本文件下载 */
import { CacheKeys, withCache } from '../cache';
import { invokeCommand } from '../invoke';
import type { VersionManifest } from '../../types/events';
import { dedupedInvoke, dedupedInvokeLong, invokeLong, withGameDirInfoInvalidation } from './internal';

export const versionApi = {
  async getVersions(): Promise<VersionManifest> {
    return withCache(CacheKeys.VERSIONS, () => dedupedInvoke<VersionManifest>('get_versions'));
  },

  async downloadVersion(versionId: string, mirror?: string): Promise<void> {
    // 下载不去重，每次都是新请求
    return withGameDirInfoInvalidation(() => invokeLong('download_version', { versionId, mirror }));
  },

  async cancelDownload(): Promise<void> {
    return invokeCommand('cancel_download');
  },

  async validateVersionFiles(versionId: string): Promise<string[]> {
    return dedupedInvokeLong<string[]>('validate_version_files', { versionId });
  },
};
