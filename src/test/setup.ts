/**
 * Vitest 全局 setup
 */
Object.defineProperty(globalThis.navigator, 'language', {
  value: 'zh-CN',
  configurable: true,
  writable: true,
});

Object.defineProperty(globalThis.navigator, 'languages', {
  value: ['zh-CN'],
  configurable: true,
  writable: true,
});

// Node 22+ 暴露了 localStorage，但没带 --localstorage-file 时取到 undefined 并打警告；
// 这里给一个内存实现，保证首屏缓存相关代码在测试里行为一致
if (!isStorageUsable()) {
  Object.defineProperty(globalThis, 'localStorage', {
    value: createMemoryStorage(),
    configurable: true,
    writable: true,
  });
}

/** 宿主自带的 localStorage 是否真的能用（Node 的占位实现会抛错或返回 undefined） */
function isStorageUsable(): boolean {
  try {
    const storage = globalThis.localStorage as Storage | undefined;
    if (!storage) return false;
    const probe = '__ar1s_probe__';
    storage.setItem(probe, '1');
    storage.removeItem(probe);
    return true;
  } catch {
    return false;
  }
}

function createMemoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    key: (index: number) => [...map.keys()][index] ?? null,
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => {
      map.set(String(key), String(value));
    },
    removeItem: (key: string) => {
      map.delete(key);
    },
    clear: () => {
      map.clear();
    },
  } as Storage;
}

// 组件测试跑在 happy-dom 下：Vuetify 的布局与虚拟滚动依赖 ResizeObserver
if (typeof window !== 'undefined' && !('ResizeObserver' in window)) {
  class ResizeObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }

  Object.defineProperty(window, 'ResizeObserver', {
    value: ResizeObserverStub,
    configurable: true,
    writable: true,
  });
}
