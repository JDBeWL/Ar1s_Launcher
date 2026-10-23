/**
 * 创建实例流程
 * 版本列表的搜索过滤排序、按版本取可用加载器、防抖校验实例名与安装进度订阅
 */
import { ref, computed, watch } from 'vue';
import { api } from '../services';
import { translate } from '../i18n';
import { useNotificationStore } from '../stores/notificationStore';
import { useSettingsStore } from '../stores/settings';
import { useDebounceFn } from './useDebounce';
import { useEventSubscription } from './useEventSubscription';
import { getErrorMessage } from '../utils/format';
import { logError } from '../utils/logger';
import { sortVersionsByReleaseTime } from '../utils/format';
import type {
  DownloadProgress,
  MinecraftVersion,
  InstallProgressPayload,
  ForgeVersion,
  LoaderVersionInfo,
  AvailableLoaders,
  ModLoaderType,
  LoaderPayload,
} from '../types/events';

export function useInstanceCreation() {
  const settingsStore = useSettingsStore();
  const versions = ref<MinecraftVersion[]>([]);
  const loadingVersions = ref(false);
  const selectedVersion = ref<MinecraftVersion | null>(null);
  const searchVersion = ref('');
  const versionTypeFilter = ref('release');
  const sortOrder = ref('newest');

  const instanceName = ref('');
  const instanceNameError = ref<string | null>(null);
  const installing = ref(false);
  const showProgress = ref(false);
  const progressValue = ref(0);
  const progressIndeterminate = ref(false);
  const progressText = ref('');

  const availableLoaders = ref<AvailableLoaders | null>(null);
  const loadingAvailableLoaders = ref(false);
  const selectedModLoaderType = ref<ModLoaderType>('None');
  const modLoaderVersions = ref<(ForgeVersion | LoaderVersionInfo)[]>([]);
  const loadingModLoaderVersions = ref(false);
  const selectedModLoaderVersion = ref<ForgeVersion | LoaderVersionInfo | null>(null);

  const modLoaderTypes = computed<{ title: string; value: ModLoaderType; disabled: boolean }[]>(() => {
    const loaders = availableLoaders.value;
    return [
      { title: translate('customInstall.loaderNone'), value: 'None' as ModLoaderType, disabled: false },
      { title: 'Forge', value: 'Forge' as ModLoaderType, disabled: !loaders?.forge },
      { title: 'Fabric', value: 'Fabric' as ModLoaderType, disabled: !loaders?.fabric },
      { title: 'Quilt', value: 'Quilt' as ModLoaderType, disabled: !loaders?.quilt },
      { title: 'NeoForge', value: 'NeoForge' as ModLoaderType, disabled: !loaders?.neoforge },
    ];
  });

  // 过滤与排序拆成两个 computed，让排序不依赖 versions 原始数组
  const filteredVersionsUnsorted = computed(() => {
    return versions.value.filter((version) => {
      const typeMatch = versionTypeFilter.value === 'all' || version.type === versionTypeFilter.value;
      const searchMatch = !searchVersion.value || version.id.toLowerCase().includes(searchVersion.value.toLowerCase());
      return typeMatch && searchMatch;
    });
  });

  const filteredVersions = computed(() => {
    if (sortOrder.value === 'newest' || sortOrder.value === 'oldest') {
      return sortVersionsByReleaseTime(filteredVersionsUnsorted.value, sortOrder.value);
    }
    return filteredVersionsUnsorted.value;
  });

  const defaultInstanceName = computed(() => {
    if (selectedVersion.value) {
      if (selectedModLoaderType.value && selectedModLoaderType.value !== 'None') {
        return `${selectedVersion.value.id}-${selectedModLoaderType.value}`;
      }
      return selectedVersion.value.id;
    }
    return '';
  });

  async function fetchVersions() {
    loadingVersions.value = true;
    try {
      const manifest = await api.version.getVersions();
      // 保留原始 releaseTime (ISO 8601) 用于排序，显示时再格式化
      versions.value = manifest.versions;
    } catch (error) {
      logError('Failed to fetch versions', error, 'useInstanceCreation');
    } finally {
      loadingVersions.value = false;
    }
  }

  async function fetchAvailableLoaders() {
    if (!selectedVersion.value) {
      availableLoaders.value = null;
      return;
    }

    loadingAvailableLoaders.value = true;
    try {
      const loaders = await api.loader.getAvailableLoaders(selectedVersion.value.id);
      availableLoaders.value = loaders;

      if (selectedModLoaderType.value !== 'None') {
        const loaderKey = selectedModLoaderType.value.toLowerCase() as keyof AvailableLoaders;
        if (!loaders[loaderKey]) {
          selectedModLoaderType.value = 'None';
        }
      }
    } catch (error) {
      logError('Failed to fetch available loaders', error, 'useInstanceCreation');
      availableLoaders.value = null;
    } finally {
      loadingAvailableLoaders.value = false;
    }
  }

  // 快速切换 MC 版本/加载器时旧请求后到会覆盖新列表，还会把选中项设成旧版本的第一条，
  // 之后点创建就把旧加载器版本号随新版本一起提交了；按发起顺序丢弃过期响应
  let modLoaderVersionsSeq = 0;

  async function fetchModLoaderVersions() {
    if (!selectedVersion.value || !selectedModLoaderType.value || selectedModLoaderType.value === 'None') {
      // 提前返回也要作废在途请求，否则它后到会把刚清空的状态又填回去
      ++modLoaderVersionsSeq;
      modLoaderVersions.value = [];
      selectedModLoaderVersion.value = null;
      return;
    }

    const seq = ++modLoaderVersionsSeq;
    loadingModLoaderVersions.value = true;
    selectedModLoaderVersion.value = null;

    try {
      const mcVersion = selectedVersion.value.id;
      const loaderType = selectedModLoaderType.value;
      let result: (ForgeVersion | LoaderVersionInfo)[] = [];

      switch (loaderType) {
        case 'Forge':
          result = await api.loader.getForgeVersions(mcVersion);
          break;
        case 'Fabric':
          result = await api.loader.getFabricVersions(mcVersion);
          break;
        case 'Quilt':
          result = await api.loader.getQuiltVersions(mcVersion);
          break;
        case 'NeoForge':
          result = await api.loader.getNeoForgeVersions(mcVersion);
          break;
      }

      if (seq !== modLoaderVersionsSeq) return;
      modLoaderVersions.value = result;

      if (result.length > 0) {
        selectedModLoaderVersion.value = result[0];
      }
    } catch (error) {
      if (seq !== modLoaderVersionsSeq) return;
      logError(`Failed to fetch ${selectedModLoaderType.value} versions`, error, 'useInstanceCreation');
      modLoaderVersions.value = [];
    } finally {
      if (seq === modLoaderVersionsSeq) {
        loadingModLoaderVersions.value = false;
      }
    }
  }

  watch(selectedVersion, () => {
    selectedModLoaderType.value = 'None';
    modLoaderVersions.value = [];
    selectedModLoaderVersion.value = null;
    fetchAvailableLoaders();
  });

  watch(selectedModLoaderType, () => {
    fetchModLoaderVersions();
  });

  async function validateInstanceName(name: string): Promise<boolean> {
    if (!name) {
      instanceNameError.value = null;
      return true; // 空名称会使用默认名称
    }

    try {
      const result = await api.instance.validateInstanceName(name);
      if (!result.is_valid) {
        instanceNameError.value = result.error_message;
        return false;
      }
      instanceNameError.value = null;
      return true;
    } catch (error) {
      logError('Failed to validate instance name', error, 'useInstanceCreation');
      instanceNameError.value = translate('customInstall.validateFailed');
      return false;
    }
  }

  const debouncedValidate = useDebounceFn((name: string) => {
    validateInstanceName(name);
  }, 300);

  watch(instanceName, (newName) => {
    debouncedValidate.call(newName);
  });

  const installProgressSub = useEventSubscription<InstallProgressPayload>('instance-install-progress', (event) => {
    const progressData = event.payload;
    progressValue.value = progressData.progress;
    progressText.value = progressData.message;
    progressIndeterminate.value = progressData.indeterminate;
  });

  const downloadProgressSub = useEventSubscription<DownloadProgress>('download-progress', (event) => {
    const data = event.payload;
    if (data.status === 'downloading') {
      progressValue.value = data.percent;
      progressIndeterminate.value = false;
      const speed = data.speed >= 1024 ? `${(data.speed / 1024).toFixed(1)} MB/s` : `${data.speed.toFixed(0)} KB/s`;
      progressText.value =
        data.speed > 0
          ? translate('customInstall.completingWithSpeed', { speed })
          : translate('customInstall.completingFiles');
    } else if (data.status === 'completed') {
      progressValue.value = 100;
      progressIndeterminate.value = false;
      progressText.value = translate('customInstall.completingDone');
    }
  });

  async function createInstance() {
    const notificationStore = useNotificationStore();

    if (!selectedVersion.value) {
      notificationStore.warning(translate('customInstall.selectVersionFirstToast'));
      return;
    }

    const finalInstanceName = instanceName.value || defaultInstanceName.value;
    if (!finalInstanceName) {
      notificationStore.warning(translate('customInstall.nameRequired'));
      return;
    }

    const isValid = await validateInstanceName(finalInstanceName);
    if (!isValid) {
      notificationStore.warning(
        translate('customInstall.nameInvalid'),
        instanceNameError.value || translate('customInstall.nameInvalidHint'),
      );
      return;
    }

    installing.value = true;
    showProgress.value = true;
    progressValue.value = 0;
    progressIndeterminate.value = true;
    progressText.value = translate('customInstall.preparingInstall');

    await installProgressSub.subscribe();

    try {
      const payload: {
        newInstanceName: string;
        baseVersionId: string;
        loader?: LoaderPayload;
      } = {
        newInstanceName: finalInstanceName,
        baseVersionId: selectedVersion.value.id,
      };

      if (selectedModLoaderVersion.value && selectedModLoaderType.value !== 'None') {
        const mcVersion = selectedVersion.value.id;
        // ForgeVersion 和 LoaderVersionInfo 都有 version 字段
        const loaderVersion =
          'version' in selectedModLoaderVersion.value
            ? selectedModLoaderVersion.value.version
            : String(selectedModLoaderVersion.value);

        switch (selectedModLoaderType.value) {
          case 'Forge':
            payload.loader = {
              type: 'forge',
              mc_version: mcVersion,
              loader_version: loaderVersion,
            };
            break;
          case 'Fabric':
            payload.loader = {
              type: 'fabric',
              mc_version: mcVersion,
              loader_version: loaderVersion,
            };
            break;
          case 'Quilt':
            payload.loader = {
              type: 'quilt',
              mc_version: mcVersion,
              loader_version: loaderVersion,
            };
            break;
          case 'NeoForge':
            payload.loader = {
              type: 'neoforge',
              mc_version: mcVersion,
              loader_version: loaderVersion,
            };
            break;
        }
      }

      await api.instance.createInstance(finalInstanceName, selectedVersion.value.id, payload.loader);

      progressValue.value = 0;
      progressText.value = translate('customInstall.validatingFiles');
      progressIndeterminate.value = true;

      let validationFailed = false;
      let missingFileCount = 0;
      try {
        const missing = await api.version.validateVersionFiles(finalInstanceName);
        missingFileCount = missing.length;
      } catch (error) {
        validationFailed = true;
        logError('Failed to validate instance files', error, 'useInstanceCreation');
      }

      let completionFailed = false;
      if (missingFileCount > 0) {
        progressValue.value = 0;
        progressIndeterminate.value = true;
        progressText.value = translate('customInstall.completingFiles');
        await downloadProgressSub.subscribe();
        try {
          const mirror = settingsStore.downloadMirror || undefined;
          await api.version.downloadVersion(finalInstanceName, mirror);
        } catch (error) {
          completionFailed = true;
          logError('Failed to complete instance files', error, 'useInstanceCreation');
          notificationStore.warning(
            translate('customInstall.createdIncomplete'),
            translate('customInstall.createdIncompleteHint'),
          );
        } finally {
          downloadProgressSub.unsubscribe();
        }
      }

      if (!completionFailed) {
        notificationStore.success(
          translate('customInstall.createSuccess'),
          validationFailed
            ? translate('customInstall.createdUnvalidated', { name: finalInstanceName })
            : translate('customInstall.createdValidated', { name: finalInstanceName }),
        );
      }

      showProgress.value = false;
      installing.value = false;
    } catch (error) {
      logError('Failed to create instance', error, 'useInstanceCreation');
      progressText.value = translate('customInstall.installFailedText');
      progressIndeterminate.value = false;
      installing.value = false;

      await new Promise((resolve) => setTimeout(resolve, 1000));
      showProgress.value = false;

      notificationStore.error(translate('customInstall.createFailed'), getErrorMessage(error), true);
    } finally {
      installProgressSub.unsubscribe();
    }
  }

  return {
    versions,
    loadingVersions,
    selectedVersion,
    searchVersion,
    versionTypeFilter,
    sortOrder,
    filteredVersions,
    instanceName,
    instanceNameError,
    defaultInstanceName,
    installing,
    showProgress,
    progressValue,
    progressIndeterminate,
    progressText,
    availableLoaders,
    loadingAvailableLoaders,
    modLoaderTypes,
    selectedModLoaderType,
    modLoaderVersions,
    loadingModLoaderVersions,
    selectedModLoaderVersion,
    fetchVersions,
    fetchAvailableLoaders,
    fetchModLoaderVersions,
    validateInstanceName,
    createInstance,
  };
}
