/**
 * 已安装版本管理
 * 读取游戏目录里的已安装版本，未选中时优先恢复上次选择
 */
import { ref, onScopeDispose } from 'vue';
import { useEventSubscription } from './useEventSubscription';
import { configApi } from '../services';
import { logError } from '../utils/logger';

export function useVersionManager() {
  const installedVersions = ref<string[]>([]);
  const selectedVersion = ref('');
  const loading = ref(false);
  const gameDir = ref('');
  // initListeners 是幂等的：HomeView 与 DownloadView 都会调用，重复挂载会叠加监听
  let focusListening = false;

  const eventSub = useEventSubscription<string>('game-dir-changed', (event) => {
    gameDir.value = event.payload;
    loadInstalledVersions();
  });

  async function loadGameDir() {
    try {
      gameDir.value = await configApi.getGameDir();
      await loadInstalledVersions();
    } catch (err) {
      logError('Failed to get game directory', err, 'useVersionManager');
    }
  }

  async function loadInstalledVersions() {
    try {
      loading.value = true;
      const dirInfo = await configApi.getGameDirInfo();
      if (dirInfo?.versions) {
        installedVersions.value = dirInfo.versions;

        if (!selectedVersion.value) {
          const lastVersion = await configApi.getLastSelectedVersion();
          if (lastVersion && installedVersions.value.includes(lastVersion)) {
            selectedVersion.value = lastVersion;
          } else if (installedVersions.value.length > 0) {
            selectedVersion.value = installedVersions.value[0];
          }
        }
      }
    } catch (err) {
      logError('Failed to get installed versions', err, 'useVersionManager');
    } finally {
      loading.value = false;
    }
  }

  async function initListeners() {
    await eventSub.subscribe();
    // 另一个实例可能刚创建/删除了版本；切回窗口时重新扫一遍目录
    if (!focusListening) {
      window.addEventListener('focus', loadGameDir);
      focusListening = true;
    }
  }

  function cleanup() {
    eventSub.unsubscribe();
    if (focusListening) {
      window.removeEventListener('focus', loadGameDir);
      focusListening = false;
    }
  }

  // 卸载时必须移除监听：在首页与下载页之间来回切会不断叠加 focus 监听，每次切回窗口触发多次 IPC
  onScopeDispose(cleanup);

  return {
    installedVersions,
    selectedVersion,
    loading,
    gameDir,
    loadGameDir,
    loadInstalledVersions,
    initListeners,
    cleanup,
  };
}
