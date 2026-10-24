import { beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope } from 'vue';

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  validateInstanceName: vi.fn(),
  createInstance: vi.fn(),
  validateVersionFiles: vi.fn(),
  downloadVersion: vi.fn(),
  getAvailableLoaders: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  success: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
vi.mock('../services', () => ({
  api: {
    instance: {
      validateInstanceName: mocks.validateInstanceName,
      createInstance: mocks.createInstance,
    },
    version: {
      validateVersionFiles: mocks.validateVersionFiles,
      downloadVersion: mocks.downloadVersion,
      getVersions: vi.fn(),
    },
    loader: { getAvailableLoaders: mocks.getAvailableLoaders },
  },
}));
vi.mock('../i18n', () => ({ translate: (key: string) => key }));
vi.mock('../utils/logger', () => ({ logError: vi.fn() }));
vi.mock('../stores/notificationStore', () => ({
  useNotificationStore: () => ({
    error: mocks.error,
    warning: mocks.warning,
    success: mocks.success,
  }),
}));
vi.mock('../stores/settings', () => ({
  useSettingsStore: () => ({ downloadMirror: 'auto' }),
}));

import { useInstanceCreation } from './useInstanceCreation';

function setup() {
  const scope = effectScope();
  const creation = scope.run(() => useInstanceCreation())!;
  creation.selectedVersion.value = { id: '1.20.1', type: 'release', releaseTime: '' } as never;
  creation.instanceName.value = 'MyPack';
  return { scope, ...creation };
}

describe('useInstanceCreation 安装流程', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.validateInstanceName.mockResolvedValue({ is_valid: true });
    mocks.getAvailableLoaders.mockResolvedValue({
      forge: false,
      fabric: false,
      quilt: false,
      neoforge: false,
    });
  });

  it('事件订阅失败时不留下安装中状态', async () => {
    mocks.listen.mockRejectedValue(new Error('listen failed'));

    const { installing, showProgress, createInstance } = setup();
    await createInstance();

    expect(mocks.createInstance).not.toHaveBeenCalled();
    expect(installing.value).toBe(false);
    expect(showProgress.value).toBe(false);
    expect(mocks.error).toHaveBeenCalledTimes(1);
  });

  it('写盘失败时复位状态并退订', async () => {
    const unlisten = vi.fn();
    mocks.listen.mockResolvedValue(unlisten);
    mocks.createInstance.mockRejectedValue(new Error('磁盘写入失败'));

    const { installing, showProgress, createInstance } = setup();
    await createInstance();

    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(installing.value).toBe(false);
    expect(showProgress.value).toBe(false);
    expect(mocks.error).toHaveBeenCalledTimes(1);
  });
});
