/**
 * 多语言入口
 *
 * 语言按「界面语言」单独保存，与游戏内语言（config 里的 language）互不影响。
 * 首屏先用 localStorage 里的值，App.vue 挂载后再用配置文件里的值覆盖，
 * 避免启动瞬间闪一下默认语言。
 */
import { createI18n } from 'vue-i18n';
import zhCN from './locales/zh-CN';
import enUS from './locales/en-US';

export const SUPPORTED_LOCALES = ['zh-CN', 'en-US'] as const;
export type UiLocale = (typeof SUPPORTED_LOCALES)[number];

export const DEFAULT_LOCALE: UiLocale = 'zh-CN';

/** localStorage 里的键名，只当首屏缓存用，配置文件才是持久化来源 */
export const UI_LOCALE_STORAGE_KEY = 'uiLanguage';

const SUPPORTED_SET: ReadonlySet<string> = new Set(SUPPORTED_LOCALES);

export function isSupportedLocale(value: unknown): value is UiLocale {
  return typeof value === 'string' && SUPPORTED_SET.has(value);
}

/** 把 zh / zh-hans / en-US / zh_CN 之类的值收敛到我们支持的两种 */
export function normalizeLocale(value: string | null | undefined): UiLocale | null {
  if (!value) return null;
  // 下划线写法（zh_CN）也常见，先统一成 BCP 47 的连字符形式再判断
  const lower = value.trim().toLowerCase().replace(/_/g, '-');
  if (lower === 'zh' || lower === 'zh-cn' || lower === 'zh-hans' || lower.startsWith('zh-')) return 'zh-CN';
  if (lower === 'en' || lower === 'en-us' || lower.startsWith('en-')) return 'en-US';
  return isSupportedLocale(value) ? value : null;
}

function systemLocale(): UiLocale | null {
  if (typeof navigator === 'undefined') return null;
  const languages = navigator.languages ?? (navigator.language ? [navigator.language] : []);
  for (const language of languages) {
    const normalized = normalizeLocale(language);
    if (normalized) return normalized;
  }
  return null;
}

export function getStoredLocale(): UiLocale | null {
  if (typeof localStorage === 'undefined') return null;
  try {
    return normalizeLocale(localStorage.getItem(UI_LOCALE_STORAGE_KEY));
  } catch {
    return null;
  }
}

export function detectLocale(): UiLocale {
  return getStoredLocale() ?? systemLocale() ?? DEFAULT_LOCALE;
}

export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  // 刻意收成 string：locale 的联合类型会让 Vuetify 的 vue-i18n 适配器对不上签名，
  // 合法值由 normalizeLocale / setLocale 把关
  locale: detectLocale() as string,
  fallbackLocale: DEFAULT_LOCALE,
  messages: {
    'zh-CN': zhCN,
    'en-US': enUS,
  },
  // 开发时缺键要看得见，生产环境不刷屏
  missingWarn: import.meta.env.DEV,
  fallbackWarn: false,
});

/** 当前界面语言 */
export function currentLocale(): UiLocale {
  return normalizeLocale(i18n.global.locale.value) ?? DEFAULT_LOCALE;
}

/**
 * 切换界面语言
 *
 * 同步 <html lang>，让浏览器与屏幕阅读器拿到正确的语言；
 * 持久化交给调用方（App.vue 写配置、GeneralSettings 立即写）。
 */
export function setLocale(locale: UiLocale): void {
  i18n.global.locale.value = locale;
  if (typeof document !== 'undefined') {
    document.documentElement.lang = locale;
  }
  try {
    localStorage.setItem(UI_LOCALE_STORAGE_KEY, locale);
  } catch {
    // 隐私模式写不了 localStorage 时只影响首屏缓存，不影响本次运行
  }
}

interface ComposerLike {
  t: (key: string, named?: Record<string, unknown>, plural?: number) => string;
}

/**
 * 在组件外（store、composable、工具函数）取翻译
 *
 * 组件里一律用模板的 `$t` 或 `useI18n()`；这里给没有 setup 上下文的地方用，
 * 读的是全局 composer，所以同样会跟随语言切换。
 */
export function translate(key: string, named?: Record<string, unknown>, plural?: number): string {
  const composer = i18n.global as unknown as ComposerLike;
  if (plural !== undefined) {
    return named ? composer.t(key, named, plural) : composer.t(key, undefined, plural);
  }
  return named ? composer.t(key, named) : composer.t(key);
}

interface ListComposerLike {
  t: (key: string, list: readonly unknown[]) => string;
  te: (key: string) => boolean;
}

/** 按位置参数取翻译（`{0}`、`{1}`），用于后端错误码带的参数组 */
export function translateList(key: string, params: readonly (string | number)[]): string {
  const composer = i18n.global as unknown as ListComposerLike;
  return composer.t(key, params);
}

/** 词条是否存在；不存在时要回退到后端原文而不是显示原始 key */
export function hasTranslation(key: string): boolean {
  const composer = i18n.global as unknown as ListComposerLike;
  return composer.te(key);
}

/** 词条原文（不做插值），用于判断它需要几个位置参数 */
export function rawTranslation(key: string): string | null {
  const composer = i18n.global as unknown as { tm: (key: string) => unknown };
  const message = composer.tm(key);
  return typeof message === 'string' ? message : null;
}
