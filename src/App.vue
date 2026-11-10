<script setup lang="ts">
import { Window } from '@tauri-apps/api/window';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { ref, watch, onMounted, onUnmounted, computed } from 'vue';
import { useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { useTheme, useDisplay } from 'vuetify';
import { useLauncherStore } from './stores/launcherStore';
import { useDownloadStore } from './stores/downloadStore';
import { useAuthStore } from './stores/authStore';
import { useSettingsStore } from './stores/settings';
import { useUiLanguageStore } from './stores/uiLanguage';
import { getThemePalette, getTheme, isValidThemePalette } from './utils/storage';
import { configApi } from './services';
import { logError } from './utils/logger';
import GlobalDownloadStatus from './components/GlobalDownloadStatus.vue';
import GlobalNotification from './components/GlobalNotification.vue';
import CrashReportDialog from './components/CrashReportDialog.vue';

const appWindow = Window.getCurrent();
const isMaximized = ref(false);
let unlistenResized: UnlistenFn | null = null;

const windowControls = {
  minimize: () => appWindow.minimize(),
  toggleMaximize: async () => {
    const maximized = await appWindow.isMaximized();
    if (maximized) {
      await appWindow.unmaximize();
    } else {
      await appWindow.maximize();
    }
  },
  close: () => appWindow.close(),
};

async function syncMaximized() {
  isMaximized.value = await appWindow.isMaximized();
}

// 用户手动切过之后，窗口宽窄变化不再覆盖 rail 状态
const display = useDisplay();
const rail = ref(!display.lgAndUp.value);
let railToggledByUser = false;

watch(
  () => display.lgAndUp.value,
  (wide) => {
    if (!railToggledByUser) rail.value = !wide;
  },
);

function toggleRail() {
  railToggledByUser = true;
  rail.value = !rail.value;
}

const theme = useTheme();
const isDarkMode = ref(getTheme() === 'dark');
const themePalette = ref(getThemePalette());

const router = useRouter();
const authStore = useAuthStore();
const settingsStore = useSettingsStore();

// 多实例：切回窗口时把另一实例可能改过的登录态和全局设置重新读一遍
function refreshCrossInstanceState() {
  void authStore.loadAuthStatus();
  void settingsStore.loadSettings();
}

function isTypingTarget(target: EventTarget | null) {
  if (!target || !(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable;
}

function handleGlobalShortcut(event: KeyboardEvent) {
  if (isTypingTarget(event.target)) return;
  const isCommand = event.ctrlKey || event.metaKey;
  if (!isCommand || event.altKey) return;

  const key = event.key.toLowerCase();
  if (key === ',') {
    event.preventDefault();
    router.push('/settings');
  } else if (key === 'd') {
    event.preventDefault();
    router.push('/download');
  } else if (key === 'n') {
    event.preventDefault();
    router.push('/add-instance');
  } else if (key === 'i') {
    event.preventDefault();
    router.push('/instance-manager');
  } else if (key === '/') {
    event.preventDefault();
    shortcutHelpVisible.value = !shortcutHelpVisible.value;
  }
}

function getThemeName() {
  return `${isDarkMode.value ? 'dark' : 'light'}-${themePalette.value}`;
}

function applyTheme() {
  theme.change(getThemeName());
  // localStorage 只当首屏缓存，配置文件才是持久化来源
  localStorage.setItem('theme', isDarkMode.value ? 'dark' : 'light');
  localStorage.setItem('themePalette', themePalette.value);
}

async function loadThemeFromConfig() {
  try {
    const [mode, palette] = await Promise.all([
      configApi.loadConfigKey('themeMode'),
      configApi.loadConfigKey('themePalette'),
    ]);
    if (mode === 'dark' || mode === 'light') isDarkMode.value = mode === 'dark';
    if (palette && isValidThemePalette(palette)) themePalette.value = palette;
  } catch (err) {
    logError('Failed to load theme settings', err, 'App');
  }
}

async function persistTheme() {
  try {
    await Promise.all([
      configApi.saveConfigKey('themeMode', isDarkMode.value ? 'dark' : 'light'),
      configApi.saveConfigKey('themePalette', themePalette.value),
    ]);
  } catch (err) {
    logError('Failed to save theme settings', err, 'App');
  }
}

function toggleTheme() {
  isDarkMode.value = !isDarkMode.value;
  applyTheme();
  void persistTheme();
}

function handlePaletteChange(event: Event) {
  const nextPalette = (event as CustomEvent<string>).detail;
  if (nextPalette) {
    themePalette.value = nextPalette;
    applyTheme();
    void persistTheme();
  }
}

const launcherStore = useLauncherStore();
const downloadStore = useDownloadStore();
const uiLanguageStore = useUiLanguageStore();
const { t } = useI18n();

const shortcutHelpVisible = ref(false);
// 描述随语言切换实时变化，不能存成静态数组
const shortcuts = computed(() => [
  { keys: 'Ctrl + ,', description: t('shell.shortcutSettings') },
  { keys: 'Ctrl + D', description: t('shell.shortcutDownload') },
  { keys: 'Ctrl + N', description: t('shell.shortcutAddInstance') },
  { keys: 'Ctrl + I', description: t('shell.shortcutInstanceList') },
  { keys: 'Ctrl + /', description: t('shell.shortcutHelp') },
]);

onMounted(async () => {
  // 主题和界面语言各自只读一个配置键，彼此没有依赖，串行等于白白多等一轮 IPC。
  // 主题仍要最先「应用」，但读取可以和语言并行
  await Promise.all([loadThemeFromConfig(), uiLanguageStore.load()]);
  applyTheme();

  await Promise.all([launcherStore.subscribe(), downloadStore.init()]);

  window.addEventListener('keydown', handleGlobalShortcut);
  window.addEventListener('theme-palette-changed', handlePaletteChange);
  window.addEventListener('focus', refreshCrossInstanceState);

  // 最大化/还原（含拖动边框放大）都会触发 resized，用它同步按钮图标
  unlistenResized = await appWindow.onResized(() => {
    void syncMaximized();
  });
  await syncMaximized();
});

onUnmounted(() => {
  launcherStore.unsubscribe();
  downloadStore.cleanup();
  window.removeEventListener('keydown', handleGlobalShortcut);
  window.removeEventListener('theme-palette-changed', handlePaletteChange);
  window.removeEventListener('focus', refreshCrossInstanceState);
  unlistenResized?.();
});
</script>

<template>
  <v-app>
    <v-navigation-drawer :rail="rail" :mobile-breakpoint="0" rail-width="64" width="220" color="surface-container">
      <v-list nav class="nav-list">
        <v-list-item prepend-icon="mdi-minecraft" :title="t('nav.launch')" to="/" class="nav-item mb-1">
          <v-tooltip activator="parent" location="right" :disabled="!rail">{{ t('nav.launch') }}</v-tooltip>
        </v-list-item>
        <v-list-item prepend-icon="mdi-download" :title="t('nav.download')" to="/download" class="nav-item mb-1">
          <v-tooltip activator="parent" location="right" :disabled="!rail">
            {{ t('nav.download') }} <kbd class="shortcut-key">Ctrl+D</kbd>
          </v-tooltip>
        </v-list-item>
        <v-list-item
          prepend-icon="mdi-plus-circle-outline"
          :title="t('nav.addInstance')"
          to="/add-instance"
          class="nav-item mb-1"
        >
          <v-tooltip activator="parent" location="right" :disabled="!rail">
            {{ t('nav.addInstance') }} <kbd class="shortcut-key">Ctrl+N</kbd>
          </v-tooltip>
        </v-list-item>
        <v-list-item
          prepend-icon="mdi-folder-multiple-outline"
          :title="t('nav.instanceList')"
          to="/instance-manager"
          class="nav-item"
        >
          <v-tooltip activator="parent" location="right" :disabled="!rail">
            {{ t('nav.instanceList') }} <kbd class="shortcut-key">Ctrl+I</kbd>
          </v-tooltip>
        </v-list-item>
      </v-list>

      <template v-slot:append>
        <v-list nav class="nav-list">
          <v-list-item prepend-icon="mdi-cog-outline" :title="t('nav.settings')" to="/settings" class="nav-item">
            <v-tooltip activator="parent" location="right" :disabled="!rail">
              {{ t('nav.settings') }} <kbd class="shortcut-key">Ctrl+,</kbd>
            </v-tooltip>
          </v-list-item>
        </v-list>
      </template>
    </v-navigation-drawer>

    <v-app-bar class="titlebar" data-tauri-drag-region elevation="0" color="surface">
      <template v-slot:prepend>
        <v-app-bar-nav-icon :aria-label="t('nav.toggleSidebar')" @click="toggleRail" data-tauri-no-drag />
      </template>
      <v-app-bar-title class="font-weight-bold">Ar1s Launcher</v-app-bar-title>
      <v-spacer />

      <v-btn
        icon
        data-tauri-no-drag
        :aria-label="isDarkMode ? t('shell.lightMode') : t('shell.darkMode')"
        @click="toggleTheme"
        class="theme-toggle-btn mr-1"
        variant="text"
      >
        <v-icon>{{ isDarkMode ? 'mdi-weather-sunny' : 'mdi-weather-night' }}</v-icon>
        <v-tooltip activator="parent" location="bottom">
          {{ isDarkMode ? t('shell.lightMode') : t('shell.darkMode') }}
        </v-tooltip>
      </v-btn>

      <v-btn
        icon
        :aria-label="t('shell.minimize')"
        data-tauri-no-drag
        @click="windowControls.minimize()"
        variant="text"
        class="window-control-btn"
      >
        <v-icon size="20">mdi-minus</v-icon>
      </v-btn>
      <v-btn
        icon
        :aria-label="isMaximized ? t('shell.restore') : t('shell.maximize')"
        data-tauri-no-drag
        @click="windowControls.toggleMaximize()"
        variant="text"
        class="window-control-btn"
      >
        <v-icon size="18">{{ isMaximized ? 'mdi-window-restore' : 'mdi-window-maximize' }}</v-icon>
      </v-btn>
      <v-btn
        icon
        :aria-label="t('shell.closeWindow')"
        data-tauri-no-drag
        @click="windowControls.close()"
        variant="text"
        class="close-btn"
      >
        <v-icon size="20">mdi-close</v-icon>
      </v-btn>
    </v-app-bar>

    <v-main>
      <router-view v-slot="{ Component }">
        <component :is="Component" />
      </router-view>
    </v-main>

    <GlobalDownloadStatus />
    <GlobalNotification />
    <CrashReportDialog />

    <v-dialog v-model="shortcutHelpVisible" max-width="380">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-title-large font-weight-bold mb-4">{{ t('shell.shortcutsTitle') }}</div>
          <div class="shortcut-list">
            <div v-for="s in shortcuts" :key="s.keys" class="shortcut-row">
              <span class="text-body-medium">{{ s.description }}</span>
              <kbd class="shortcut-key-lg">{{ s.keys }}</kbd>
            </div>
          </div>
        </v-card-text>
        <v-card-actions class="pa-4 pt-0">
          <v-spacer />
          <v-btn variant="text" @click="shortcutHelpVisible = false">{{ t('common.close') }}</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </v-app>
</template>

<style>
:root {
  color-scheme: light dark;
}

.v-application {
  background-color: rgb(var(--v-theme-background)) !important;
}

.v-main {
  --v-layout-top: 0px !important;
  height: calc(100vh - 64px);
  margin-top: 64px;
  overflow-y: auto;
  overflow-x: hidden;
}

::-webkit-scrollbar {
  width: 8px;
  height: 8px;
}

::-webkit-scrollbar-track {
  background: transparent;
}

::-webkit-scrollbar-thumb {
  background: rgba(var(--v-theme-on-surface), 0.2);
  border-radius: 4px;
}

::-webkit-scrollbar-thumb:hover {
  background: rgba(var(--v-theme-on-surface), 0.35);
}

.titlebar .v-toolbar__content {
  pointer-events: none;
}

.titlebar .v-btn,
.titlebar .v-app-bar-nav-icon {
  pointer-events: auto;
}

.nav-list {
  padding: 8px;
}

.v-list-item.nav-item {
  --v-list-prepend-gap: 0;
  padding: 0 12px;
  min-height: 48px;
  margin-bottom: 4px;
  overflow: hidden;
}

.nav-item > .v-list-item__prepend {
  margin-inline-end: 12px;
}

.nav-item.v-list-item--active {
  background: rgb(var(--v-theme-secondary-container));
  color: rgb(var(--v-theme-on-secondary-container));
}

.nav-item.v-list-item--active .v-icon {
  color: rgb(var(--v-theme-on-secondary-container));
}

/* rail 模式只隐藏文字，布局与展开模式完全一致 */
.v-navigation-drawer--rail .nav-item > .v-list-item__content {
  display: none;
}

.theme-toggle-btn {
  transform: rotate(0deg);
}

.theme-toggle-btn:hover {
  transform: rotate(30deg);
}

.titlebar .close-btn:hover {
  background-color: rgb(var(--v-theme-error));
  color: rgb(var(--v-theme-on-error));
}

.titlebar .window-control-btn:hover {
  background-color: rgba(var(--v-theme-on-surface), 0.08);
}

.v-app .surface-container {
  background-color: rgb(var(--v-theme-surface-container));
}

.v-app .surface-container-high {
  background-color: rgb(var(--v-theme-surface-container-high));
}

.v-app .surface-container-highest {
  background-color: rgb(var(--v-theme-surface-container-highest));
}

.shortcut-key {
  display: inline-block;
  padding: 1px 5px;
  margin-left: 6px;
  font-size: 11px;
  font-family: inherit;
  line-height: 1.4;
  background: rgba(var(--v-theme-on-surface), 0.12);
  border-radius: 4px;
  color: rgba(var(--v-theme-on-surface), 0.7);
}

.shortcut-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.shortcut-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.shortcut-key-lg {
  display: inline-block;
  padding: 2px 8px;
  font-size: 12px;
  font-family: inherit;
  line-height: 1.5;
  background: rgba(var(--v-theme-on-surface), 0.08);
  border: 1px solid rgba(var(--v-theme-on-surface), 0.15);
  border-radius: 6px;
  color: rgba(var(--v-theme-on-surface), 0.8);
}
</style>
