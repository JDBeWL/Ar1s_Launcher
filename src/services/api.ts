/**
 * Tauri 后端接口调用模块
 * 按域封装 invoke，相同参数的并发请求合并，只读结果走缓存、写操作后按前缀失效
 */
import { invokeCommand } from './invoke';
import { withCache, cache, CacheKeys } from './cache';
import type {
  VersionManifest,
  GameInstance,
  ModInfo,
  AvailableLoaders,
  ForgeVersion,
  LoaderVersionInfo,
  InstanceNameValidation,
  GameDirInfo,
  ModrinthVersion,
  ModrinthSearchResult,
  ModInstallResult,
  ModUpdate,
  WindowSettings,
  JvmAdvancedOptions,
  AuthStatus,
  DeviceCodeDisplay,
  MicrosoftLoginResult,
  JavaCompatibilityResult,
  GameConfig,
  InstanceSettings,
  MemoryStats,
  SaveInfo,
  SaveBackupInfo,
  SaveImportSummary,
  ContentTarget,
  CrashReportInfo,
  CrashReportText,
  LaunchRecord,
  LogTail,
  ProxySettings,
  ProxyTestResult,
  ResourcePackInfo,
  ResourcePackImportSummary,
  InstalledResourcePack,
  ResourcePackUpdate,
  ShaderPackList,
  ShaderPackImportSummary,
  InstalledShaderPack,
  ShaderPackUpdate,
  LocalModpackPreview,
  ModpackExportOptions,
  ModpackExportResult,
  CfManualFile,
  CfCategory,
  CfProjectRef,
} from '../types/events';

const pendingRequests = new Map<string, Promise<unknown>>();

function invalidateGameDirInfo(): void {
  cache.deleteByPrefix(`${CacheKeys.CONFIG}:game_dir_info`);
}

/** 安装/下载会改动 versions 目录，前后都失效，避免事件监听读到旧缓存 */
async function withGameDirInfoInvalidation<T>(operation: () => Promise<T>): Promise<T> {
  invalidateGameDirInfo();
  try {
    return await operation();
  } finally {
    invalidateGameDirInfo();
  }
}

async function dedupedInvoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
  options?: { skipDedup?: boolean },
): Promise<T> {
  if (options?.skipDedup) {
    return invokeCommand<T>(cmd, args);
  }

  const key = `${cmd}:${JSON.stringify(args ?? {})}`;
  if (pendingRequests.has(key)) {
    return pendingRequests.get(key) as Promise<T>;
  }

  const promise = invokeCommand<T>(cmd, args).finally(() => {
    pendingRequests.delete(key);
  });

  pendingRequests.set(key, promise);
  return promise;
}

export const versionApi = {
  async getVersions(): Promise<VersionManifest> {
    return withCache(CacheKeys.VERSIONS, () => dedupedInvoke<VersionManifest>('get_versions'));
  },

  async downloadVersion(versionId: string, mirror?: string): Promise<void> {
    // 下载不去重，每次都是新请求
    return withGameDirInfoInvalidation(() => invokeCommand('download_version', { versionId, mirror }));
  },

  async cancelDownload(): Promise<void> {
    return invokeCommand('cancel_download');
  },

  async validateVersionFiles(versionId: string): Promise<string[]> {
    return dedupedInvoke<string[]>('validate_version_files', { versionId });
  },
};

export const instanceApi = {
  async getInstances(): Promise<GameInstance[]> {
    return dedupedInvoke<GameInstance[]>('get_instances');
  },

  async createInstance(
    newInstanceName: string,
    baseVersionId: string,
    loader?: { type: string; mc_version: string; loader_version: string },
  ): Promise<void> {
    return withGameDirInfoInvalidation(() =>
      invokeCommand('create_instance', { newInstanceName, baseVersionId, loader }),
    );
  },

  async deleteInstance(instanceName: string): Promise<void> {
    return withGameDirInfoInvalidation(() => invokeCommand('delete_instance', { instanceName }));
  },

  async renameInstance(oldName: string, newName: string): Promise<void> {
    return withGameDirInfoInvalidation(() => invokeCommand('rename_instance', { oldName, newName }));
  },

  async openInstanceFolder(instanceName: string): Promise<void> {
    return invokeCommand('open_instance_folder', { instanceName });
  },

  /** mods 目录里的模组，含已禁用的；状态会变，不走缓存 */
  async getMods(instanceName: string): Promise<ModInfo[]> {
    return invokeCommand<ModInfo[]>('get_instance_mods', { instanceName });
  },

  /** 启用/禁用模组（改名 *.jar <-> *.jar.disabled） */
  async setModEnabled(instanceName: string, fileName: string, enabled: boolean): Promise<void> {
    await invokeCommand('set_mod_enabled', { instanceName, fileName, enabled });
  },

  async deleteMod(instanceName: string, fileName: string): Promise<void> {
    await invokeCommand('delete_instance_mod', { instanceName, fileName });
  },

  /** 导出脱敏诊断包；目标路径由原生保存对话框选定 */
  async exportDiagnostics(instanceName: string, targetPath: string): Promise<void> {
    await invokeCommand('export_diagnostic_bundle', { instanceName, targetPath });
  },

  async launchInstance(instanceName: string, overrideJavaPath?: string, forceJavaMismatch?: boolean): Promise<void> {
    await invokeCommand('launch_instance', { instanceName, overrideJavaPath, forceJavaMismatch });
  },

  async getInstanceSettings(instanceName: string): Promise<InstanceSettings> {
    return dedupedInvoke<InstanceSettings>('get_instance_settings', { instanceName });
  },

  async setInstanceSettings(instanceName: string, settings: InstanceSettings): Promise<void> {
    await invokeCommand('set_instance_settings', { instanceName, settings });
  },

  async validateInstanceName(name: string): Promise<InstanceNameValidation> {
    return dedupedInvoke<InstanceNameValidation>('validate_instance_name_cmd', { name });
  },

  async checkInstanceNameAvailable(name: string): Promise<InstanceNameValidation> {
    return dedupedInvoke<InstanceNameValidation>('check_instance_name_available', { name });
  },
};

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

export const javaApi = {
  async findJavaInstallations(): Promise<string[]> {
    return withCache(CacheKeys.JAVA, () => dedupedInvoke<string[]>('find_java_installations_command'), 10 * 60 * 1000);
  },

  async refreshJavaInstallations(): Promise<string[]> {
    const result = await invokeCommand<string[]>('refresh_java_installations');
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
    return invokeCommand<JavaCompatibilityResult>('check_java_compatibility', { mcVersion });
  },
};

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
      () => dedupedInvoke<GameDirInfo>('get_game_dir_info'),
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

export const saveApi = {
  async getSaves(instanceName: string): Promise<SaveInfo[]> {
    return invokeCommand<SaveInfo[]>('get_instance_saves', { instanceName });
  },

  async getBackups(instanceName: string): Promise<SaveBackupInfo[]> {
    return invokeCommand<SaveBackupInfo[]>('get_save_backups', { instanceName });
  },

  async backup(instanceName: string, world: string): Promise<SaveBackupInfo> {
    return invokeCommand<SaveBackupInfo>('backup_save', { instanceName, world });
  },

  async restore(instanceName: string, world: string, fileName: string): Promise<void> {
    await invokeCommand('restore_save_backup', { instanceName, world, fileName });
  },

  async deleteBackup(instanceName: string, world: string, fileName: string): Promise<void> {
    await invokeCommand('delete_save_backup', { instanceName, world, fileName });
  },

  async importSaves(instanceName: string, sourcePaths: string[]): Promise<SaveImportSummary> {
    return invokeCommand<SaveImportSummary>('import_saves', { instanceName, sourcePaths });
  },

  async openSavesFolder(instanceName: string): Promise<void> {
    await invokeCommand('open_saves_folder', { instanceName });
  },
};

export const logApi = {
  async getHistory(instanceName: string): Promise<LaunchRecord[]> {
    return invokeCommand<LaunchRecord[]>('get_launch_history', { instanceName });
  },

  async getLatestLog(instanceName: string): Promise<LogTail> {
    return invokeCommand<LogTail>('get_latest_log', { instanceName });
  },

  async getLaunchLog(instanceName: string, launchId: number): Promise<LogTail> {
    return invokeCommand<LogTail>('get_launch_log', { instanceName, launchId });
  },

  async getCrashReports(instanceName: string): Promise<CrashReportInfo[]> {
    return invokeCommand<CrashReportInfo[]>('get_crash_reports', { instanceName });
  },

  async getCrashReport(instanceName: string, fileName: string): Promise<CrashReportText> {
    return invokeCommand<CrashReportText>('get_crash_report', { instanceName, fileName });
  },

  async openContent(instanceName: string, target: ContentTarget): Promise<void> {
    await invokeCommand('open_instance_content', { instanceName, target });
  },
};

export const userApi = {
  async getSavedUsername(): Promise<string | null> {
    return dedupedInvoke<string | null>('get_saved_username');
  },

  async setSavedUsername(username: string): Promise<void> {
    return invokeCommand('set_saved_username', { username });
  },

  async getSavedUuid(): Promise<string | null> {
    return dedupedInvoke<string | null>('get_saved_uuid');
  },

  async setSavedUuid(uuid: string): Promise<void> {
    return invokeCommand('set_saved_uuid', { uuid });
  },

  async getAuthStatus(): Promise<AuthStatus> {
    return dedupedInvoke<AuthStatus>('get_auth_status');
  },

  async startMicrosoftLogin(): Promise<DeviceCodeDisplay> {
    return invokeCommand<DeviceCodeDisplay>('start_microsoft_login');
  },

  /** 轮询等待用户授权 */
  async completeMicrosoftLogin(): Promise<MicrosoftLoginResult> {
    return invokeCommand<MicrosoftLoginResult>('complete_microsoft_login');
  },

  /** Authorization Code + PKCE 流程，返回浏览器授权 URL */
  async startMicrosoftAuthCodeLogin(): Promise<string> {
    return invokeCommand<string>('start_microsoft_auth_code_login');
  },

  /** 等浏览器回调，而非轮询 */
  async completeMicrosoftAuthCodeLogin(): Promise<MicrosoftLoginResult> {
    return invokeCommand<MicrosoftLoginResult>('complete_microsoft_auth_code_login');
  },

  async cancelMicrosoftLogin(): Promise<void> {
    return invokeCommand('cancel_microsoft_login');
  },

  async refreshMicrosoftAuth(): Promise<MicrosoftLoginResult> {
    return invokeCommand<MicrosoftLoginResult>('refresh_microsoft_auth');
  },

  /** 换皮肤不会触发 token 刷新，需用已存 access_token 重新取地址 */
  async refreshSkinUrl(): Promise<string | null> {
    return invokeCommand<string | null>('refresh_skin_url');
  },

  async logoutMicrosoft(): Promise<void> {
    return invokeCommand('logout_microsoft');
  },

  async setAuthType(authType: string): Promise<void> {
    return invokeCommand('set_auth_type', { authType });
  },
};

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
    return withGameDirInfoInvalidation(() => invokeCommand('install_modrinth_modpack', { options }));
  },

  /** 读取本地 .mrpack 的清单，安装页据此展示包信息 */
  async previewLocalModpack(filePath: string): Promise<LocalModpackPreview> {
    return invokeCommand<LocalModpackPreview>('preview_local_modpack', { filePath });
  },

  /** 从本地压缩包安装（.mrpack 与 CurseForge zip 自动识别），压缩包不会被移动或删除；
   * 返回值是 CurseForge 受限文件的手动下载清单，Modrinth 格式恒为空 */
  async installLocalModpack(options: { filePath: string; instanceName: string }): Promise<CfManualFile[]> {
    return withGameDirInfoInvalidation(() =>
      invokeCommand<CfManualFile[]>('install_local_modpack', {
        options: { file_path: options.filePath, instance_name: options.instanceName },
      }),
    );
  },

  /** 把实例打包成 Modrinth 格式的 .mrpack；目标路径由原生保存对话框选定 */
  async exportModpack(instanceName: string, options: ModpackExportOptions): Promise<ModpackExportResult> {
    // options 走 serde camelCase，其余命令参数按 Tauri 的 snake_case->camelCase 约定
    return invokeCommand<ModpackExportResult>('export_modpack', { instanceName, options });
  },

  async cancelModpackInstall(): Promise<void> {
    return invokeCommand('cancel_modpack_install');
  },
};

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
    return invokeCommand<ModInstallResult>('install_modrinth_mod', {
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
    return invokeCommand<ModInstallResult>('update_modrinth_mod', {
      instanceName,
      fileName,
      versionId,
    });
  },
};

/**
 * CurseForge 数据源（麦块开放 API 中转，响应与 Modrinth 命令同构）
 */
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
    return invokeCommand<ModInstallResult>('install_curseforge_mod', {
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
    return invokeCommand<CfProjectRef>('resolve_curseforge_modpack', { input });
  },

  /** 返回需手动下载的受限文件清单，安装本体成功时为空数组 */
  async installModpack(options: {
    modpack_id: string;
    version_id: string;
    instance_name: string;
    install_path: string;
  }): Promise<CfManualFile[]> {
    return withGameDirInfoInvalidation(() => invokeCommand<CfManualFile[]>('install_curseforge_modpack', { options }));
  },
};

export const resourcePackApi = {
  /** 实例 resourcepacks 目录里的资源包，含启用状态；状态会变，不走缓存 */
  async getResourcePacks(instanceName: string): Promise<ResourcePackInfo[]> {
    return invokeCommand<ResourcePackInfo[]>('get_resource_packs', { instanceName });
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
    return invokeCommand<ResourcePackImportSummary>('import_resource_packs', { instanceName, sourcePaths });
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
    return invokeCommand<InstalledResourcePack>('install_modrinth_resource_pack', {
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
    return invokeCommand<InstalledResourcePack>('update_modrinth_resource_pack', {
      instanceName,
      fileName,
      versionId,
    });
  },
};

export const shaderPackApi = {
  /** 实例 shaderpacks 目录里的光影包，含配置里的选中状态；状态会变，不走缓存 */
  async getShaderPacks(instanceName: string): Promise<ShaderPackList> {
    return invokeCommand<ShaderPackList>('get_shader_packs', { instanceName });
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
    return invokeCommand<ShaderPackImportSummary>('import_shader_packs', { instanceName, sourcePaths });
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
    return invokeCommand<InstalledShaderPack>('install_modrinth_shader_pack', {
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
    return invokeCommand<InstalledShaderPack>('update_modrinth_shader_pack', {
      instanceName,
      fileName,
      versionId,
    });
  },
};

export const api = {
  version: versionApi,
  instance: instanceApi,
  loader: loaderApi,
  java: javaApi,
  config: configApi,
  save: saveApi,
  log: logApi,
  user: userApi,
  modpack: modpackApi,
  mod: modApi,
  curseforge: curseforgeApi,
  resourcePack: resourcePackApi,
  shaderPack: shaderPackApi,
};

export default api;
