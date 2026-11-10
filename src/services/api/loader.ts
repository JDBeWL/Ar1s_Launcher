/** 各加载器可安装版本查询 */
import { CacheKeys, withCache } from '../cache';
import type { AvailableLoaders, ForgeVersion, LoaderVersionInfo } from '../../types/events';
import { dedupedInvoke } from './internal';

export const loaderApi = {
  async getAvailableLoaders(minecraftVersion: string): Promise<AvailableLoaders> {
    return withCache(`${CacheKeys.LOADERS}:${minecraftVersion}`, () =>
      dedupedInvoke<AvailableLoaders>('get_available_loaders', { minecraftVersion }),
    );
  },

  async getForgeVersions(minecraftVersion: string): Promise<ForgeVersion[]> {
    return withCache(`${CacheKeys.LOADERS}:forge:${minecraftVersion}`, () =>
      dedupedInvoke<ForgeVersion[]>('get_forge_versions', { minecraftVersion }),
    );
  },

  async getFabricVersions(minecraftVersion: string): Promise<LoaderVersionInfo[]> {
    return withCache(`${CacheKeys.LOADERS}:fabric:${minecraftVersion}`, () =>
      dedupedInvoke<LoaderVersionInfo[]>('get_fabric_versions', { minecraftVersion }),
    );
  },

  async getQuiltVersions(minecraftVersion: string): Promise<LoaderVersionInfo[]> {
    return withCache(`${CacheKeys.LOADERS}:quilt:${minecraftVersion}`, () =>
      dedupedInvoke<LoaderVersionInfo[]>('get_quilt_versions', { minecraftVersion }),
    );
  },

  async getNeoForgeVersions(minecraftVersion: string): Promise<LoaderVersionInfo[]> {
    return withCache(`${CacheKeys.LOADERS}:neoforge:${minecraftVersion}`, () =>
      dedupedInvoke<LoaderVersionInfo[]>('get_neoforge_versions', { minecraftVersion }),
    );
  },
};
