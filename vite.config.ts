import { defineConfig } from 'vite';
import type { Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import vuetify from 'vite-plugin-vuetify';
import * as path from 'path';
import { existsSync, readFileSync, readdirSync } from 'fs';

const host = process.env.TAURI_DEV_HOST;

const pkgVersion = JSON.parse(readFileSync(path.resolve(import.meta.dirname, 'package.json'), 'utf-8')).version;

/**
 * WebView2 for woff2
 */
function mdiWoff2Only(): Plugin {
  return {
    name: 'mdi-woff2-only',
    enforce: 'pre',
    transform(code, id) {
      if (!id.includes('materialdesignicons.css')) return null;

      const replaced = code.replace(
        /@font-face\s*\{[^}]*\}/,
        `@font-face {
  font-family: "Material Design Icons";
  src: url("../fonts/materialdesignicons-webfont.woff2?v=7.4.47") format("woff2");
  font-weight: normal;
  font-style: normal;
}`,
      );

      // 没匹配上说明包结构变了，宁可保留原样也不要悄悄改坏图标字体
      if (replaced === code) {
        this.warn('未能匹配 materialdesignicons.css 的 @font-face，字体体积未优化');
        return null;
      }
      return replaced;
    },
  };
}

/**
 * 裁掉 `@mdi/font` 里用不到的图标规则。
 *
 * Vuetify 组件内部有一整套默认图标（下拉箭头、复选框、分页按钮），那些名字
 * 只存在于 `vuetify/lib/iconsets/mdi`，只扫本项目 `src/` 会把它们全裁掉，表现就是按钮图标空白。
 */
function mdiUsedIconsOnly(): Plugin {
  let allowed: Set<string> | null = null;

  /** 从 Vuetify 的 mdi 图标集里取全部别名值（'mdi-chevron-down' 这种） */
  function collectVuetifyIcons(): Set<string> {
    const names = new Set<string>();
    const libRoot = path.resolve(import.meta.dirname, 'node_modules/vuetify/lib');
    if (!existsSync(libRoot)) return names;

    const iconset = path.join(libRoot, 'iconsets/mdi.js');
    if (existsSync(iconset)) {
      const source = readFileSync(iconset, 'utf-8');
      for (const match of source.matchAll(/'mdi-[a-z0-9-]+'/g)) {
        names.add(match[0].slice(1, -1));
      }
    }

    // 2. 组件里硬编码的图标字符串（排除 iconsets，避免重复扫）
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          if (entry.name !== 'iconsets') walk(full);
        } else if (entry.name.endsWith('.js')) {
          const source = readFileSync(full, 'utf-8');
          for (const match of source.matchAll(/'mdi-[a-z0-9-]+'/g)) {
            names.add(match[0].slice(1, -1));
          }
        }
      }
    };
    walk(libRoot);

    return names;
  }

  /** 本项目源码里用到的图标 */
  function collectProjectIcons(): Set<string> {
    const names = new Set<string>();
    const srcRoot = path.resolve(import.meta.dirname, 'src');
    if (!existsSync(srcRoot)) return names;

    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          walk(full);
        } else if (/\.(vue|ts|css|html)$/.test(entry.name)) {
          const source = readFileSync(full, 'utf-8');
          for (const match of source.matchAll(/\bmdi-[a-z0-9-]+\b/g)) {
            names.add(match[0]);
          }
        }
      }
    };
    walk(srcRoot);

    return names;
  }

  return {
    name: 'mdi-used-icons-only',
    enforce: 'pre',
    transform(code, id) {
      if (!id.includes('materialdesignicons.css')) return null;
      // 只处理源文件（带 @font-face 的那种大文件），已经压缩过的产物不再动
      if (!code.includes('@font-face') || !code.includes('::before')) return null;

      if (allowed === null) {
        allowed = new Set([...collectVuetifyIcons(), ...collectProjectIcons()]);
        // mdi-set 是类名而非图标，字体定义本身也可能被别处引用
        allowed.add('mdi-set');
      }

      const kept = code.replace(/\.mdi-([a-z0-9-]+)::before\s*\{\s*content\s*:[^}]*\}/g, (rule, name: string) =>
        allowed!.has(`mdi-${name}`) ? rule : '',
      );

      const before = (code.match(/\.mdi-[a-z0-9-]+::before\s*\{\s*content/g) ?? []).length;
      const after = (kept.match(/\.mdi-[a-z0-9-]+::before\s*\{\s*content/g) ?? []).length;

      // 自检：一条都没裁掉，说明规则格式变了，保留原样并告警
      if (before > 0 && after === before) {
        this.warn('MDI 图标规则未匹配到任何可裁剪项，保持原样');
        return null;
      }

      return kept;
    },
  };
}

export default defineConfig(async () => ({
  plugins: [mdiWoff2Only(), mdiUsedIconsOnly(), vue(), vuetify({ autoImport: true })],
  define: {
    __APP_VERSION__: JSON.stringify(pkgVersion),
  },
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },

  test: {
    globals: true,
    environment: 'node',
    include: ['src/**/*.{test,spec}.{ts,tsx}', 'scripts/**/*.test.mjs'],
    setupFiles: ['./src/test/setup.ts'],
    // Vuetify 的组件入口自带 .css 导入，交给 Vite 处理而不是让 Node 直接加载
    server: {
      deps: {
        inline: ['vuetify'],
      },
    },
  },

  clearScreen: false,
  // Tauri 要求固定端口，端口被占用时必须直接失败
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // src-tauri 由 Rust 侧编译，不能让 Vite watch 它
      ignored: ['**/src-tauri/**'],
    },
  },
}));
