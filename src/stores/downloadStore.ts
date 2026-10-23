/**
 * 版本下载状态存储
 * 订阅 download-progress 维护进度与错误，取消失败时回退成 emit 事件
 */
import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import { emit } from '@tauri-apps/api/event';
import type { DownloadProgress, DownloadStatus } from '../types/events';
import { api } from '../services/api';
import { translate } from '../i18n';
import { useNotificationStore } from './notificationStore';
import { getErrorMessage, getErrorCode } from '../utils/format';
import { logError, logInfo } from '../utils/logger';

export type StoreDownloadStatus = DownloadStatus | 'idle';

export interface DownloadState extends Omit<DownloadProgress, 'status'> {
  status: StoreDownloadStatus;
}

let downloadProgressUnlisten: (() => void) | null = null;

export const useDownloadStore = defineStore('download', () => {
  const selectedVersion = ref('');
  const downloadError = ref<string | null>(null);
  const downloadProgress = ref<DownloadState>({
    bytes_downloaded: 0,
    total_bytes: 0,
    speed: 0,
    status: 'idle',
    percent: 0,
    error: undefined,
  });
  const isDownloading = computed(() => downloadProgress.value.status === 'downloading');
  const completionNotified = ref(false);
  const showNotification = ref(false);
  const userHidNotification = ref(false);
  const isInitialized = ref(false);
  // 只有下载页主动发起的下载才归这个 store 管；整合包/实例创建/修复的内部下载不弹全局窗
  const ownsDownload = ref(false);
  // 后端按游戏目录加下载锁：任何流程的下载期间再点下载只会白报一个占用错误。
  // download-progress 不区分发起方，全局记一笔在忙，startDownload 据此挡在前面
  const backendBusy = ref(false);

  function handleDownloadProgress(event: DownloadProgress) {
    backendBusy.value = event.status === 'downloading';
    if (!ownsDownload.value) return;

    const data = event as DownloadState;
    downloadProgress.value = data;

    if (data.status === 'downloading' && !userHidNotification.value) {
      showNotification.value = true;
    }

    if (data.status === 'completed' && !completionNotified.value) {
      completionNotified.value = true;
      if (!userHidNotification.value) {
        showNotification.value = true;
      }
    } else if (data.status === 'cancelled' || data.status === 'error') {
      completionNotified.value = false;
      selectedVersion.value = '';
      userHidNotification.value = false;
      if (data.status === 'error') {
        downloadError.value = data.error || translate('downloadStatus.unknownError');
        const notificationStore = useNotificationStore();
        notificationStore.error(
          translate('downloadStatus.title'),
          data.error || translate('downloadStatus.unknownError'),
          true,
        );
      }
    }

    if (data.status !== 'downloading') {
      ownsDownload.value = false;
    }
  }

  async function init() {
    if (isInitialized.value) return;

    try {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<DownloadProgress>('download-progress', (event) => {
        handleDownloadProgress(event.payload);
      });
      downloadProgressUnlisten = unlisten;
      isInitialized.value = true;
      logInfo('Download store initialized', 'DownloadStore');
    } catch (err) {
      logError('Failed to initialize download store', err, 'DownloadStore');
    }
  }

  function cleanup() {
    if (downloadProgressUnlisten) {
      downloadProgressUnlisten();
      downloadProgressUnlisten = null;
      isInitialized.value = false;
    }
  }

  async function startDownload(versionId: string, source: string = 'auto') {
    if (isDownloading.value || backendBusy.value) {
      const notificationStore = useNotificationStore();
      notificationStore.warning(translate('downloadStatus.alreadyRunning'));
      return;
    }

    selectedVersion.value = versionId;
    downloadProgress.value.status = 'downloading';
    downloadError.value = null;
    completionNotified.value = false;
    userHidNotification.value = false;
    showNotification.value = true;
    ownsDownload.value = true;

    try {
      await api.version.downloadVersion(selectedVersion.value, source);
    } catch (err) {
      logError('Failed to start download invocation', err, 'DownloadStore');
      ownsDownload.value = false;
      if (getErrorCode(err) === 'cancelled') {
        downloadProgress.value.status = 'cancelled';
        downloadError.value = null;
        return;
      }
      downloadProgress.value.status = 'error';
      const errorMessage = getErrorMessage(err);
      downloadError.value = errorMessage;
      const notificationStore = useNotificationStore();
      notificationStore.error(translate('downloadStatus.title'), errorMessage, true);
    }
  }

  async function cancelDownload() {
    try {
      await api.version.cancelDownload();
    } catch (err) {
      logError('Failed to cancel download', err, 'DownloadStore');
      await emit('cancel-download');
    }
  }

  function toggleNotification() {
    showNotification.value = !showNotification.value;
  }

  function showDownloadNotification() {
    showNotification.value = true;
  }

  function hideDownloadNotification() {
    showNotification.value = false;
    userHidNotification.value = true;
  }

  return {
    selectedVersion,
    downloadProgress,
    isDownloading,
    completionNotified,
    downloadError,
    showNotification,
    userHidNotification,

    init,
    cleanup,
    startDownload,
    cancelDownload,
    toggleNotification,
    showDownloadNotification,
    hideDownloadNotification,
  };
});
