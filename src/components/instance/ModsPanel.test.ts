// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { defineComponent } from 'vue';

/**
 * 面板直接用 services 里的三个 API；这里整体替换掉，避免真的走 Tauri invoke。
 * vi.hoisted 保证 mock 工厂先于组件模块求值。
 */
const services = vi.hoisted(() => ({
  configApi: { loadConfigKey: vi.fn() },
  instanceApi: {
    getMods: vi.fn(),
    setModEnabled: vi.fn(),
    deleteMod: vi.fn(),
  },
  modApi: {
    checkModUpdates: vi.fn(),
    updateMod: vi.fn(),
    updateAllMods: vi.fn(),
  },
}));

vi.mock('../../services', () => services);
// 失败路径会写一条错误日志，测试里没必要连带打一屏堆栈
vi.mock('../../utils/logger', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../utils/logger')>();
  return { ...actual, logError: vi.fn(), logWarn: vi.fn() };
});

import ModsPanel from './ModsPanel.vue';
import { mountComponent, settle } from '../../test/mount';
import { useNotificationStore } from '../../stores/notificationStore';
import type { ModInfo } from '../../types/events';

/** v-virtual-scroll 依赖真实布局，测试里换成「全部渲染」的桩 */
const VirtualScrollStub = defineComponent({
  name: 'VVirtualScroll',
  props: {
    items: { type: Array, default: () => [] },
  },
  methods: {
    scrollToIndex() {},
  },
  template: `<div class="virtual-scroll-stub">
    <template v-for="item in items" :key="item.fileName">
      <slot :item="item" />
    </template>
  </div>`,
});

const MODS: ModInfo[] = [
  { fileName: 'sodium.jar', displayName: 'Sodium', version: '0.5.8', enabled: true, size: 1024 },
  { fileName: 'iris.jar', displayName: 'Iris Shaders', version: null, enabled: false, size: 2048 },
];

function mountPanel() {
  return mountComponent(ModsPanel, {
    props: { instanceName: 'Test Instance', gameVersion: '1.20.1', loader: 'fabric' },
    global: { stubs: { 'v-virtual-scroll': VirtualScrollStub } },
  });
}

describe('ModsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    services.configApi.loadConfigKey.mockResolvedValue('true');
    services.instanceApi.getMods.mockResolvedValue(MODS.map((mod) => ({ ...mod })));
    services.instanceApi.setModEnabled.mockResolvedValue(undefined);
  });

  it('挂载后加载并渲染模组列表', async () => {
    const wrapper = mountPanel();
    await settle();

    expect(services.instanceApi.getMods).toHaveBeenCalledWith('Test Instance');
    expect(wrapper.text()).toContain('Sodium');
    expect(wrapper.text()).toContain('Iris Shaders');
  });

  it('切换开关时按相反状态写回并通知父级刷新', async () => {
    const wrapper = mountPanel();
    await settle();

    const switches = wrapper.findAll('input[type="checkbox"]');
    expect(switches.length).toBe(2);

    // 第二个是禁用状态的 Iris，点一下应当启用
    await switches[1].setValue(true);
    await settle();

    expect(services.instanceApi.setModEnabled).toHaveBeenCalledWith('Test Instance', 'iris.jar', true);
    expect(wrapper.emitted('changed')).toHaveLength(1);
  });

  it('写回失败时提示错误且不改动列表状态', async () => {
    services.instanceApi.setModEnabled.mockRejectedValue(new Error('磁盘只读'));
    const wrapper = mountPanel();
    await settle();

    const store = useNotificationStore();
    const errorSpy = vi.spyOn(store, 'error');

    const switches = wrapper.findAll('input[type="checkbox"]');
    await switches[1].setValue(true);
    await settle();

    expect(errorSpy).toHaveBeenCalled();
    expect(wrapper.emitted('changed')).toBeUndefined();
  });

  it('搜索框按名称过滤列表', async () => {
    const wrapper = mountPanel();
    await settle();

    const search = wrapper.find('input[type="text"]');
    await search.setValue('sod');
    await settle();

    expect(wrapper.text()).toContain('Sodium');
    expect(wrapper.text()).not.toContain('Iris Shaders');
  });
});
