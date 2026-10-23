<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { useSettingsStore } from '../../stores/settings';
import { useUiLanguageStore } from '../../stores/uiLanguage';
import { configApi } from '../../services';
import { logError } from '../../utils/logger';
import { getThemePalette } from '../../utils/storage';
import ThemePalettePicker from './ThemePalettePicker.vue';
import NetworkProxySettings from './NetworkProxySettings.vue';

const settingsStore = useSettingsStore();
const uiLanguageStore = useUiLanguageStore();
const { t } = useI18n();

const gameDir = ref('');
const versionIsolation = ref(true);
const downloadThreads = ref(16);
const downloadThreadsInput = ref('16');
const isInitialized = ref(false);
const appVersion = __APP_VERSION__;
const themePalette = ref(getThemePalette());

// 界面语言用 nativeName 展示，游戏内语言用各自语言的写法
const interfaceLanguages = uiLanguageStore.languages.map((item) => ({ title: item.nativeName, value: item.value }));

const downloadMirrorOptions = computed(() => [
  { title: t('settings.general.mirrorAuto'), value: 'auto' },
  { title: t('settings.general.mirrorOfficial'), value: 'official' },
  { title: t('settings.general.mirrorMirror'), value: 'mirror' },
]);

const gameLanguage = ref('zh_cn');
const gameLanguages = [
  { title: '简体中文', value: 'zh_cn' },
  { title: '繁體中文', value: 'zh_tw' },
  { title: 'English (US)', value: 'en_us' },
  { title: 'English (UK)', value: 'en_gb' },
  { title: '日本語', value: 'ja_jp' },
  { title: '한국어', value: 'ko_kr' },
  { title: 'Français', value: 'fr_fr' },
  { title: 'Deutsch', value: 'de_de' },
  { title: 'Español', value: 'es_es' },
  { title: 'Italiano', value: 'it_it' },
  { title: 'Português (BR)', value: 'pt_br' },
  { title: 'Русский', value: 'ru_ru' },
];

let unlistenGameDirChanged: UnlistenFn | null = null;

async function loadGameDir() {
  try {
    gameDir.value = await configApi.getGameDir();
  } catch (err) {
    logError('Failed to get game directory', err, 'GeneralSettings');
  }
}

async function selectGameDir() {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t('settings.general.gameDirDialog'),
    });
    if (selected) {
      gameDir.value = selected as string;
      await configApi.setGameDir(gameDir.value);
    }
  } catch (err) {
    logError('Failed to select directory', err, 'GeneralSettings');
  }
}

async function loadDownloadThreads() {
  try {
    downloadThreads.value = clampDownloadThreads(await configApi.getDownloadThreads());
    downloadThreadsInput.value = String(downloadThreads.value);
  } catch (err) {
    logError('Failed to get download threads', err, 'GeneralSettings');
  }
}

function clampDownloadThreads(value: number): number {
  return Math.min(256, Math.max(1, Math.round(value)));
}

async function saveDownloadThreads() {
  try {
    await configApi.setDownloadThreads(downloadThreads.value);
  } catch (err) {
    logError('Failed to set download threads', err, 'GeneralSettings');
  }
}

function commitDownloadThreads() {
  const raw = downloadThreadsInput.value.trim();
  const parsed = raw === '' ? Number.NaN : Number(raw);
  if (!Number.isFinite(parsed)) {
    downloadThreadsInput.value = String(downloadThreads.value);
    return;
  }

  const clamped = clampDownloadThreads(parsed);
  downloadThreads.value = clamped;
  downloadThreadsInput.value = String(clamped);
  void saveDownloadThreads();
}

async function loadVersionIsolation() {
  try {
    const isolation = await configApi.loadConfigKey('versionIsolation');
    if (isolation !== null) {
      versionIsolation.value = isolation === 'true';
    }
  } catch (err) {
    logError('Failed to load isolation settings', err, 'GeneralSettings');
  }
}

async function saveIsolationSetting(key: string, value: boolean) {
  try {
    await configApi.saveConfigKey(key, value.toString());
  } catch (err) {
    logError(`Failed to save ${key}`, err, 'GeneralSettings');
  }
}

// onMounted 的 load 会给这些 ref 赋值，没有守卫就会一进页面把原值回写一遍
watch(versionIsolation, (v) => {
  if (!isInitialized.value) return;
  saveIsolationSetting('versionIsolation', v);
});
watch(gameLanguage, async (v) => {
  if (!isInitialized.value) return;
  try {
    await configApi.saveConfigKey('language', v);
  } catch (err) {
    logError('Failed to save language', err, 'GeneralSettings');
  }
});

watch(
  () => settingsStore.downloadMirror,
  async () => {
    if (!isInitialized.value) return;
    await settingsStore.saveDownloadMirror();
  },
);

watch(downloadThreads, (value) => {
  downloadThreadsInput.value = String(value);
});

watch(themePalette, (value) => {
  localStorage.setItem('themePalette', value);
  window.dispatchEvent(new CustomEvent('theme-palette-changed', { detail: value }));
  // 落盘交给 App.vue 的 theme-palette-changed 处理，它同时写配置和缓存
});

onMounted(async () => {
  await loadGameDir();
  await loadDownloadThreads();
  await loadVersionIsolation();
  await settingsStore.loadSettings();

  try {
    const lang = await configApi.loadConfigKey('language');
    if (lang) gameLanguage.value = lang;
  } catch (err) {
    logError('Failed to load language', err, 'GeneralSettings');
  }

  isInitialized.value = true;

  unlistenGameDirChanged = await listen('game-dir-changed', (event) => {
    gameDir.value = event.payload as string;
  });
});

onUnmounted(() => {
  if (unlistenGameDirChanged) {
    unlistenGameDirChanged();
    unlistenGameDirChanged = null;
  }
});
</script>

<template>
  <div class="settings-group">
    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-3">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-folder-outline</v-icon>
          <span class="text-title-medium">{{ t('settings.general.gameDirTitle') }}</span>
        </div>
        <v-text-field
          density="comfortable"
          v-model="gameDir"
          :label="t('settings.general.gameDirLabel')"
          readonly
          hide-details
        >
          <template #append-inner>
            <v-btn
              icon
              variant="text"
              size="x-small"
              :aria-label="t('settings.general.gameDirAria')"
              @click="selectGameDir"
            >
              <v-icon size="18">mdi-folder-open-outline</v-icon>
            </v-btn>
          </template>
        </v-text-field>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-1">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-folder-multiple-outline</v-icon>
            <span class="text-title-medium">{{ t('settings.general.isolationTitle') }}</span>
          </div>
          <v-switch
            v-model="versionIsolation"
            :aria-label="t('settings.general.isolationTitle')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>
        <p class="text-body-medium text-on-surface-variant mb-0">
          {{ t('settings.general.isolationHint') }}
        </p>

        <v-alert v-if="!versionIsolation" type="warning" variant="tonal" density="compact" class="mt-3 text-body-small">
          {{ t('settings.general.isolationWarning') }}
        </v-alert>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-download-outline</v-icon>
          <span class="text-title-medium">{{ t('settings.general.downloadTitle') }}</span>
        </div>

        <div class="mb-5">
          <div class="d-flex align-center justify-space-between mb-2">
            <span class="text-body-medium">{{ t('settings.general.downloadThreads') }}</span>
            <v-text-field
              v-model="downloadThreadsInput"
              type="number"
              density="compact"
              :aria-label="t('settings.general.downloadThreads')"
              :suffix="t('settings.general.downloadThreadsSuffix')"
              hide-details
              hide-spin-buttons
              class="download-threads-input"
              @blur="commitDownloadThreads"
              @keyup.enter="commitDownloadThreads"
            />
          </div>
          <v-slider
            v-model="downloadThreads"
            :min="1"
            :max="256"
            :step="1"
            hide-details
            thumb-label
            color="primary"
            @end="saveDownloadThreads"
          >
            <template #prepend>
              <span class="text-body-small text-on-surface-variant">1</span>
            </template>
            <template #append>
              <span class="text-body-small text-on-surface-variant">256</span>
            </template>
          </v-slider>
          <p class="text-body-small text-on-surface-variant mt-2 mb-0">
            {{ t('settings.general.downloadThreadsHint') }}
          </p>
        </div>

        <div>
          <div class="text-body-medium mb-3">{{ t('settings.general.mirrorTitle') }}</div>
          <v-select
            v-model="settingsStore.downloadMirror"
            :items="downloadMirrorOptions"
            :aria-label="t('settings.general.mirrorTitle')"
            density="comfortable"
            variant="outlined"
            hide-details
          />
          <p class="text-body-small text-on-surface-variant mt-2 mb-0">
            {{ t('settings.general.mirrorHint') }}
          </p>
        </div>
      </v-card-text>
    </v-card>

    <NetworkProxySettings />

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-translate</v-icon>
          <span class="text-title-medium">{{ t('settings.uiLanguage.title') }}</span>
        </div>
        <v-select
          density="comfortable"
          :model-value="uiLanguageStore.current"
          :items="interfaceLanguages"
          item-title="title"
          item-value="value"
          :label="t('settings.uiLanguage.label')"
          hide-details
          @update:model-value="uiLanguageStore.setLanguage"
        />
        <div class="text-body-small text-on-surface-variant mt-2">{{ t('settings.uiLanguage.hint') }}</div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-translate</v-icon>
          <span class="text-title-medium">{{ t('settings.general.gameLanguageTitle') }}</span>
        </div>
        <v-select
          density="comfortable"
          v-model="gameLanguage"
          :items="gameLanguages"
          item-title="title"
          item-value="value"
          :label="t('settings.general.gameLanguageLabel')"
          hide-details
        />
        <div class="text-body-small text-on-surface-variant mt-2">
          {{ t('settings.general.gameLanguageHint') }}
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-palette-outline</v-icon>
          <span class="text-title-medium">{{ t('settings.general.appearanceTitle') }}</span>
        </div>
        <ThemePalettePicker v-model="themePalette" />
        <div class="text-body-small text-on-surface-variant mt-3">
          {{ t('settings.general.appearanceHint') }}
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-information-outline</v-icon>
          <span class="text-title-medium">{{ t('settings.general.aboutTitle') }}</span>
        </div>
        <div class="d-flex align-center justify-space-between mb-2">
          <div class="text-body-medium text-on-surface-variant">{{ t('settings.general.appVersion') }}</div>
          <v-chip size="small" color="primary" variant="tonal">{{ appVersion }}</v-chip>
        </div>
        <div class="text-body-small text-on-surface-variant">
          {{ t('settings.general.shortcuts') }}
        </div>
      </v-card-text>
    </v-card>
  </div>
</template>

<style scoped>
.download-threads-input {
  max-width: 140px;
}
</style>
