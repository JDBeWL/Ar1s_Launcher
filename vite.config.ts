import { defineConfig } from 'vite';
import type { Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import vuetify from 'vite-plugin-vuetify';
import * as path from 'path';
import { readFileSync } from 'fs';

const host = process.env.TAURI_DEV_HOST;

const pkgVersion = JSON.parse(readFileSync(path.resolve(import.meta.dirname, 'package.json'), 'utf-8')).version;

/**
 * @mdi/font 的 CSS 给同一个字体声明了 eot / ttf / woff / woff2 四种格式，
 * Vite 会把四个文件全部打进产物（ttf 与 eot 各 1.2 MB，合计约 2.5 MB），
 * 而 Tauri 用的 WebView2 只认 woff2。
 *
 * 这里在 CSS 交给 Vite 处理之前，把那个 @font-face 换成只带 woff2 的版本；
 * 图标类名（.mdi-xxx:before 的码点）原样保留，不受影响
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

export default defineConfig(async () => ({
  plugins: [mdiWoff2Only(), vue(), vuetify({ autoImport: true })],
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
