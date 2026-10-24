import { beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope } from 'vue';

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  downloadVersion: vi.fn(),
  validateVersionFiles: vi.fn(),
  launchInstance: vi.fn(),
  checkJavaCompatibility: vi.fn(),
  confirm: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
  info: vi.fn(),
  warning: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
vi.mock('../services', () => ({
  versionApi: {
    downloadVersion: mocks.downloadVersion,
    validateVersionFiles: mocks.validateVersionFiles,
  },
  instanceApi: { launchInstance: mocks.launchInstance },
  javaApi: { checkJavaCompatibility: mocks.checkJavaCompatibility },
}));
vi.mock('../i18n', () => ({ translate: (key: string) => key }));
vi.mock('../utils/logger', () => ({ logError: vi.fn() }));
vi.mock('../stores/notificationStore', () => ({
  useNotificationStore: () => ({
    confirm: mocks.confirm,
    error: mocks.error,
    success: mocks.success,
    info: mocks.info,
    warning: mocks.warning,
  }),
}));
vi.mock('../stores/settings', () => ({
  useSettingsStore: () => ({ downloadMirror: 'auto' }),
}));
vi.mock('../stores/authStore', () => ({
  useAuthStore: () => ({
    isLoggedIn: true,
    authType: 'offline',
    msLoggedIn: false,
    isTokenExpired: false,
    tryRefreshMicrosoftToken: vi.fn(),
  }),
}));

import { useGameLaunch } from './useGameLaunch';

function setup() {
  const scope = effectScope();
  const launch = scope.run(() => useGameLaunch())!;
  return { scope, ...launch };
}

describe('useGameLaunch 文件修复', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.checkJavaCompatibility.mockResolvedValue({ compatible: true });
    mocks.validateVersionFiles.mockResolvedValue(['missing.jar']);
    mocks.confirm.mockResolvedValue(true);
  });

  it('事件订阅失败时不留下修复中状态', async () => {
    mocks.listen.mockRejectedValue(new Error('listen failed'));

    const { isRepairing, repairProgress, launchInstance } = setup();
    const launched = await launchInstance('A');

    expect(launched).toBe(false);
    expect(mocks.downloadVersion).not.toHaveBeenCalled();
    expect(isRepairing.value).toBe(false);
    expect(repairProgress.value).toBeNull();
    expect(mocks.error).toHaveBeenCalledTimes(1);
  });

  it('写盘失败时复位状态并退订', async () => {
    const unlisten = vi.fn();
    mocks.listen.mockResolvedValue(unlisten);
    mocks.downloadVersion.mockRejectedValue(new Error('磁盘写入失败'));

    const { isRepairing, repairProgress, launchInstance } = setup();
    const launched = await launchInstance('A');

    expect(launched).toBe(false);
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(isRepairing.value).toBe(false);
    expect(repairProgress.value).toBeNull();
    expect(mocks.error).toHaveBeenCalledTimes(1);
  });
});
