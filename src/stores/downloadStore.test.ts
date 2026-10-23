import { describe, it, expect, vi, beforeEach } from 'vitest';
import { setActivePinia, createPinia } from 'pinia';
import { listen } from '@tauri-apps/api/event';
import { useDownloadStore } from './downloadStore';
import * as api from '../services/api';
import type { DownloadProgress } from '../types/events';

vi.mock('../services/api', () => ({
  api: {
    version: {
      downloadVersion: vi.fn(),
      cancelDownload: vi.fn(),
    },
  },
}));

vi.mock('./notificationStore', () => ({
  useNotificationStore: () => ({
    error: vi.fn(),
  }),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(() => Promise.resolve()),
}));

function progress(overrides: Partial<DownloadProgress> = {}): DownloadProgress {
  return {
    bytes_downloaded: 0,
    total_bytes: 0,
    speed: 0,
    status: 'downloading',
    percent: 0,
    ...overrides,
  };
}

async function getDownloadHandler() {
  const store = useDownloadStore();
  await store.init();
  const handler = vi.mocked(listen).mock.calls[0]?.[1];
  if (!handler) throw new Error('download-progress listener not registered');
  return handler as (event: { payload: DownloadProgress }) => void;
}

describe('DownloadStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it('初始状态正确', () => {
    const store = useDownloadStore();

    expect(store.selectedVersion).toBe('');
    expect(store.downloadProgress.status).toBe('idle');
    expect(store.isDownloading).toBe(false);
    expect(store.showNotification).toBe(false);
  });

  it('startDownload 设置状态并调用 API', async () => {
    const store = useDownloadStore();
    const mockDownload = vi.mocked(api.api.version.downloadVersion).mockResolvedValue(undefined);

    await store.startDownload('1.20.1', 'mirror');

    expect(store.selectedVersion).toBe('1.20.1');
    expect(store.downloadProgress.status).toBe('downloading');
    expect(store.downloadError).toBeNull();
    expect(mockDownload).toHaveBeenCalledWith('1.20.1', 'mirror');
  });

  it('startDownload 失败时设置错误状态', async () => {
    const store = useDownloadStore();
    const error = new Error('Download failed');
    vi.mocked(api.api.version.downloadVersion).mockRejectedValue(error);

    await store.startDownload('1.20.1');

    expect(store.downloadProgress.status).toBe('error');
    expect(store.downloadError).toBeTruthy();
  });

  it('取消错误标记为 cancelled，不当作失败', async () => {
    const store = useDownloadStore();
    vi.mocked(api.api.version.downloadVersion).mockRejectedValue({
      code: 'cancelled',
      message: '操作已取消',
    });

    await store.startDownload('1.20.1');

    expect(store.downloadProgress.status).toBe('cancelled');
    expect(store.downloadError).toBeNull();
  });

  it('cancelDownload 调用 API', async () => {
    const store = useDownloadStore();
    const mockCancel = vi.mocked(api.api.version.cancelDownload).mockResolvedValue(undefined);

    await store.cancelDownload();

    expect(mockCancel).toHaveBeenCalled();
  });

  it('toggleNotification 切换通知显示', () => {
    const store = useDownloadStore();

    expect(store.showNotification).toBe(false);

    store.toggleNotification();
    expect(store.showNotification).toBe(true);

    store.toggleNotification();
    expect(store.showNotification).toBe(false);
  });

  it('hideDownloadNotification 隐藏并标记用户已隐藏', () => {
    const store = useDownloadStore();

    store.hideDownloadNotification();

    expect(store.showNotification).toBe(false);
    expect(store.userHidNotification).toBe(true);
  });

  it('未由 store 发起的下载不占用全局通知', async () => {
    const store = useDownloadStore();
    const handler = await getDownloadHandler();

    handler({ payload: progress({ bytes_downloaded: 10, total_bytes: 100, percent: 10 }) });

    expect(store.downloadProgress.status).toBe('idle');
    expect(store.isDownloading).toBe(false);
    expect(store.showNotification).toBe(false);
  });

  it('store 发起的下载仍会更新并弹出通知', async () => {
    const store = useDownloadStore();
    vi.mocked(api.api.version.downloadVersion).mockResolvedValue(undefined);
    await store.startDownload('1.20.1');
    const handler = await getDownloadHandler();

    handler({ payload: progress({ bytes_downloaded: 10, total_bytes: 100, percent: 10 }) });

    expect(store.isDownloading).toBe(true);
    expect(store.showNotification).toBe(true);
    expect(store.downloadProgress.bytes_downloaded).toBe(10);
  });

  it('完成后不再接管后续下载事件', async () => {
    const store = useDownloadStore();
    vi.mocked(api.api.version.downloadVersion).mockResolvedValue(undefined);
    await store.startDownload('1.20.1');
    const handler = await getDownloadHandler();

    handler({ payload: progress({ bytes_downloaded: 10, total_bytes: 100, percent: 10 }) });
    handler({
      payload: progress({
        bytes_downloaded: 100,
        total_bytes: 100,
        percent: 100,
        status: 'completed',
      }),
    });
    handler({ payload: progress({ bytes_downloaded: 20, total_bytes: 100, percent: 20 }) });

    expect(store.downloadProgress.status).toBe('completed');
    expect(store.downloadProgress.bytes_downloaded).toBe(100);
  });
});
