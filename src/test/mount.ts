/**
 * 组件测试挂载辅助
 *
 * 组件模板里用的是全局注册的 Vuetify 组件（构建时由 vite-plugin-vuetify 自动引入），
 * 测试里必须显式注册整套组件，否则 <v-card> 之类会被当成未知元素，DOM 查不到内容。
 */
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import type { Component } from 'vue';
import { createVuetify } from 'vuetify';
import * as vuetifyComponents from 'vuetify/components';
import * as vuetifyDirectives from 'vuetify/directives';
import { i18n, setLocale } from '../i18n';

interface MountArgs {
  props?: Record<string, unknown>;
  global?: Record<string, unknown>;
}

export function mountComponent(component: Component, args: MountArgs = {}): VueWrapper {
  const pinia = createPinia();
  setActivePinia(pinia);
  setLocale('zh-CN');

  const global = args.global ?? {};
  // 每个组件的 props 类型都不同，这里只关心渲染结果，不做逐组件泛型推导
  return mount(component, {
    ...(args as object),
    global: {
      ...global,
      plugins: [
        pinia,
        i18n,
        createVuetify({ components: vuetifyComponents, directives: vuetifyDirectives }),
        ...((global.plugins as unknown[]) ?? []),
      ],
    },
  } as never) as VueWrapper;
}

/** 等一轮微任务与渲染，用于「挂载后 onMounted 里的异步加载」 */
export async function settle(): Promise<void> {
  await flushPromises();
  await new Promise((resolve) => setTimeout(resolve, 0));
  await flushPromises();
}
