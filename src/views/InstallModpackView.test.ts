// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';

const route = vi.hoisted(() => ({ query: {} as Record<string, string> }));
const router = vi.hoisted(() => ({ back: vi.fn(), push: vi.fn() }));

vi.mock('vue-router', () => ({
  useRoute: () => route,
  useRouter: () => router,
  onBeforeRouteLeave: () => {},
}));

const services = vi.hoisted(() => ({
  modpackApi: {
    getModrinthModpackVersions: vi.fn(),
    previewLocalModpack: vi.fn(),
    installModrinthModpack: vi.fn(),
    installLocalModpack: vi.fn(),
    cancelModpackInstall: vi.fn(),
  },
  instanceApi: { checkInstanceNameAvailable: vi.fn() },
  versionApi: {
    validateVersionFiles: vi.fn(),
    downloadVersion: vi.fn(),
    cancelDownload: vi.fn(),
  },
  curseforgeApi: { getModpackVersions: vi.fn(), installModpack: vi.fn() },
}));

vi.mock('../services', () => services);
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
// 失败路径会写一条错误日志，测试里没必要连带打一屏堆栈
vi.mock('../utils/logger', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/logger')>();
  return { ...actual, logError: vi.fn(), logWarn: vi.fn() };
});

import InstallModpackView from './InstallModpackView.vue';
import { mountComponent, settle } from '../test/mount';
import { useNotificationStore } from '../stores/notificationStore';
import type { ModrinthVersion } from '../types/events';

function makeVersion(overrides: Partial<ModrinthVersion>): ModrinthVersion {
  return {
    id: 'version-id',
    name: 'Integrated Pack',
    version_number: '1.0.0',
    game_versions: ['1.20.1'],
    loaders: ['fabric'],
    featured: false,
    date_published: '2026-01-01T00:00:00Z',
    downloads: 100,
    files: [],
    dependencies: [],
    ...overrides,
  };
}

const VERSIONS: ModrinthVersion[] = [
  makeVersion({ id: 'forge-1', version_number: '1.0.0', game_versions: ['1.19.2'], loaders: ['forge'] }),
  makeVersion({ id: 'fabric-2', version_number: '2.0.0', game_versions: ['1.20.1'], loaders: ['fabric'] }),
];

const PREVIEW = {
  path: 'D:/packs/demo.mrpack',
  name: 'Demo Pack',
  versionId: 'demo-1.0.0',
  summary: null,
  minecraft: '1.20.1',
  loader: 'fabric',
  loaderVersion: '0.15.0',
  fileCount: 12,
  totalSize: 1024 * 1024,
  suggestedInstanceName: 'Demo Pack',
};

describe('InstallModpackView', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    route.query = {};
    services.instanceApi.checkInstanceNameAvailable.mockResolvedValue({
      is_valid: true,
      error_message: null,
    });
    services.versionApi.validateVersionFiles.mockResolvedValue([]);
    services.modpackApi.installModrinthModpack.mockResolvedValue(undefined);
    services.versionApi.downloadVersion.mockResolvedValue(undefined);
  });

  it('在线安装：加载版本、默认选中第一个加载器并完成安装', async () => {
    route.query = { projectId: 'proj-1', title: 'Integrated Pack' };
    services.modpackApi.getModrinthModpackVersions.mockResolvedValue(VERSIONS);

    const wrapper = mountComponent(InstallModpackView);
    await settle();

    expect(services.modpackApi.getModrinthModpackVersions).toHaveBeenCalledWith('proj-1');
    // 加载器优先级 forge > fabric，所以默认选中 1.19.2 的 forge 版本
    expect(wrapper.find('.install-btn').attributes('disabled')).toBeUndefined();

    await wrapper.find('.install-btn').trigger('click');
    await settle();

    expect(services.modpackApi.installModrinthModpack).toHaveBeenCalledWith({
      modpack_id: 'proj-1',
      version_id: 'forge-1',
      instance_name: 'Integrated Pack',
      install_path: '',
    });
    expect(services.versionApi.validateVersionFiles).toHaveBeenCalledWith('Integrated Pack');
    expect(router.push).toHaveBeenCalledWith('/instance-manager');
  });

  it('安装失败时提示错误，停留在当前页面', async () => {
    route.query = { projectId: 'proj-1', title: 'Integrated Pack' };
    services.modpackApi.getModrinthModpackVersions.mockResolvedValue(VERSIONS);
    services.modpackApi.installModrinthModpack.mockRejectedValue(new Error('磁盘空间不足：需要约 2.0 GB'));

    const wrapper = mountComponent(InstallModpackView);
    await settle();

    const store = useNotificationStore();
    const errorSpy = vi.spyOn(store, 'error');

    await wrapper.find('.install-btn').trigger('click');
    await settle();

    expect(errorSpy).toHaveBeenCalledWith(expect.any(String), expect.stringContaining('磁盘空间不足'), true);
    expect(router.push).not.toHaveBeenCalled();
  });

  it('取消安装走 info 提示，不当成失败', async () => {
    route.query = { projectId: 'proj-1', title: 'Integrated Pack' };
    services.modpackApi.getModrinthModpackVersions.mockResolvedValue(VERSIONS);
    services.modpackApi.installModrinthModpack.mockRejectedValue({
      code: 'cancelled',
      message: '操作已取消',
    });

    const wrapper = mountComponent(InstallModpackView);
    await settle();

    const store = useNotificationStore();
    const infoSpy = vi.spyOn(store, 'info');
    const errorSpy = vi.spyOn(store, 'error');

    await wrapper.find('.install-btn').trigger('click');
    await settle();

    expect(infoSpy).toHaveBeenCalled();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('本地整合包：读取预览后按建议实例名安装', async () => {
    route.query = { file: 'D:/packs/demo.mrpack' };
    services.modpackApi.previewLocalModpack.mockResolvedValue(PREVIEW);
    services.modpackApi.installLocalModpack.mockResolvedValue([]);

    const wrapper = mountComponent(InstallModpackView);
    await settle();

    expect(services.modpackApi.previewLocalModpack).toHaveBeenCalledWith('D:/packs/demo.mrpack');
    expect(wrapper.text()).toContain('待下载文件');
    expect(wrapper.text()).toContain('1.20.1');

    await wrapper.find('.install-btn').trigger('click');
    await settle();

    expect(services.modpackApi.installLocalModpack).toHaveBeenCalledWith({
      filePath: 'D:/packs/demo.mrpack',
      instanceName: 'Demo Pack',
    });
  });

  it('实例名被占用时禁用安装按钮', async () => {
    route.query = { projectId: 'proj-1', title: 'Integrated Pack' };
    services.modpackApi.getModrinthModpackVersions.mockResolvedValue(VERSIONS);
    services.instanceApi.checkInstanceNameAvailable.mockResolvedValue({
      is_valid: false,
      error_message: '实例名称已存在',
    });

    const wrapper = mountComponent(InstallModpackView);
    await settle();

    expect(wrapper.text()).toContain('实例名称已存在');
    expect(wrapper.find('.install-btn').attributes('disabled')).toBeDefined();
  });
});
