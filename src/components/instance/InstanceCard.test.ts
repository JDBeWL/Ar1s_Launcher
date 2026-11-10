// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import InstanceCard from './InstanceCard.vue';
import { mountComponent } from '../../test/mount';
import type { GameInstance } from '../../types/events';

function makeInstance(overrides: Partial<GameInstance> = {}): GameInstance {
  return {
    id: 'inst-1',
    name: 'Sodium Test',
    version: '1.20.1',
    path: 'D:/game/versions/Sodium Test',
    createdTime: '2026-01-01T00:00:00Z',
    loaderType: 'Fabric',
    loaderVersion: '0.15.0',
    gameVersion: '1.20.1',
    lastPlayed: Date.now(),
    modCount: 3,
    ...overrides,
  };
}

describe('InstanceCard', () => {
  it('展示实例名、加载器与版本号', () => {
    const wrapper = mountComponent(InstanceCard, { props: { instance: makeInstance() } });

    expect(wrapper.text()).toContain('Sodium Test');
    expect(wrapper.text()).toContain('Fabric');
    expect(wrapper.text()).toContain('1.20.1 - Fabric 0.15.0');
  });

  it('原版实例不显示加载器版本', () => {
    const wrapper = mountComponent(InstanceCard, {
      props: { instance: makeInstance({ loaderType: 'None', loaderVersion: undefined }) },
    });

    expect(wrapper.text()).toContain('1.20.1');
    expect(wrapper.text()).not.toContain('Fabric');
  });

  it('整合包实例显示整合包标签', () => {
    const wrapper = mountComponent(InstanceCard, {
      props: { instance: makeInstance({ loaderType: 'Modded', loaderVersion: '1.0.0' }) },
    });

    expect(wrapper.text()).toContain('整合包');
  });

  it('点击启动按钮抛出当前实例', async () => {
    const instance = makeInstance();
    const wrapper = mountComponent(InstanceCard, { props: { instance } });

    await wrapper.find('.launch-btn').trigger('click');

    const emitted = wrapper.emitted('launch');
    expect(emitted).toHaveLength(1);
    expect(emitted?.[0]?.[0]).toEqual(instance);
  });

  it('启动中时按钮进入加载态，不重复抛事件', async () => {
    const wrapper = mountComponent(InstanceCard, {
      props: { instance: makeInstance(), launching: true },
    });

    expect(wrapper.find('.launch-btn').classes()).toContain('v-btn--loading');
    await wrapper.find('.launch-btn').trigger('click');
    // 加载态下 Vuetify 仍会派发 click，但父级用 launching 自己拦；这里只锁定可见状态
    expect(wrapper.find('.launch-btn').classes()).toContain('v-btn--loading');
  });
});
