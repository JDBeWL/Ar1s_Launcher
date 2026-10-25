<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { convertFileSrc } from '@tauri-apps/api/core';
import { useI18n } from 'vue-i18n';
import { useInstanceScope } from '../../composables/useInstanceScope';
import { useNotificationStore } from '../../stores/notificationStore';
import { configApi, screenshotApi } from '../../services';
import type { ScreenshotInfo } from '../../types/events';
import { formatFileSize, getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const props = defineProps<{ instanceName: string }>();
const { t } = useI18n();
const notificationStore = useNotificationStore();
const scope = useInstanceScope(() => props.instanceName);

const screenshots = ref<ScreenshotInfo[]>([]);
const loading = ref(true);
const error = ref('');
const versionIsolation = ref(true);
const selected = ref<ScreenshotInfo | null>(null);
const failedImages = ref<Set<string>>(new Set());
const copying = ref(false);

const previewOpen = computed({
  get: () => selected.value !== null,
  set: (open: boolean) => {
    if (!open) selected.value = null;
  },
});

async function load() {
  const task = scope.begin();
  loading.value = true;
  error.value = '';
  try {
    const [list, isolationValue] = await Promise.all([
      screenshotApi.getScreenshots(task.instanceName),
      configApi.loadConfigKey('versionIsolation'),
    ]);
    if (!scope.isCurrent(task)) return;
    screenshots.value = list;
    versionIsolation.value = isolationValue !== 'false';
    failedImages.value = new Set();
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load screenshots', e, 'ScreenshotsPanel');
    error.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) loading.value = false;
  }
}

function imageUrl(screenshot: ScreenshotInfo) {
  return convertFileSrc(screenshot.path);
}

function formatCapturedAt(timestamp: number) {
  return new Date(timestamp).toLocaleString(undefined, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  });
}

function imageFailed(fileName: string) {
  return failedImages.value.has(fileName);
}

function markImageFailed(fileName: string) {
  failedImages.value = new Set(failedImages.value).add(fileName);
}

async function copySelected() {
  if (!selected.value || copying.value) return;

  copying.value = true;
  try {
    await screenshotApi.copyScreenshot(props.instanceName, selected.value.fileName);
    notificationStore.success(t('screenshots.copySuccess'), t('screenshots.copySuccessHint'));
  } catch (e) {
    logError('Failed to copy screenshot', e, 'ScreenshotsPanel');
    notificationStore.error(t('screenshots.copyFailed'), getErrorMessage(e));
  } finally {
    copying.value = false;
  }
}

async function openFolder() {
  try {
    await screenshotApi.openFolder(props.instanceName);
  } catch (e) {
    logError('Failed to open screenshots folder', e, 'ScreenshotsPanel');
    notificationStore.error(t('screenshots.openFolderFailed'), getErrorMessage(e));
  }
}

onMounted(load);
watch(
  () => props.instanceName,
  () => {
    selected.value = null;
    failedImages.value = new Set();
    screenshots.value = [];
    void load();
  },
);
</script>

<template>
  <div class="screenshots-panel">
    <div class="d-flex align-center ga-2 mb-3 flex-wrap">
      <v-btn color="primary" variant="tonal" size="small" prepend-icon="mdi-folder-open-outline" @click="openFolder">
        {{ t('screenshots.openFolder') }}
      </v-btn>
      <v-spacer />
      <v-btn icon size="small" variant="text" :aria-label="t('screenshots.refresh')" :loading="loading" @click="load">
        <v-icon size="18">mdi-refresh</v-icon>
      </v-btn>
    </div>

    <v-alert v-if="!versionIsolation" type="warning" variant="tonal" density="comfortable" class="mb-3">
      {{ t('screenshots.isolationWarning') }}
    </v-alert>

    <v-card variant="flat" color="surface-container">
      <div v-if="loading && screenshots.length === 0" class="d-flex align-center justify-center pa-10">
        <v-progress-circular indeterminate size="28" color="primary" />
      </div>

      <div v-else-if="error" class="text-center pa-8">
        <div class="text-body-medium text-error mb-3">{{ error }}</div>
        <v-btn size="small" variant="tonal" color="primary" @click="load">
          {{ t('common.retry') }}
        </v-btn>
      </div>

      <div v-else-if="screenshots.length === 0" class="text-center pa-10 text-on-surface-variant">
        <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-monitor</v-icon>
        <div class="text-body-medium">{{ t('screenshots.emptyTitle') }}</div>
        <div class="text-body-small mt-1">{{ t('screenshots.emptyHint') }}</div>
      </div>

      <template v-else>
        <div class="screenshot-grid pa-3">
          <v-card
            v-for="shot in screenshots"
            :key="shot.fileName"
            variant="flat"
            color="surface-container-high"
            class="screenshot-card"
            tabindex="0"
            role="button"
            @click="selected = shot"
            @keydown.enter="selected = shot"
            @keydown.space.prevent="selected = shot"
          >
            <div class="screenshot-image">
              <img
                v-if="!imageFailed(shot.fileName)"
                :src="imageUrl(shot)"
                :alt="shot.fileName"
                loading="lazy"
                decoding="async"
                @error="markImageFailed(shot.fileName)"
              />
              <div v-else class="screenshot-broken">
                <v-icon size="28" color="on-surface-variant">mdi-alert-circle-outline</v-icon>
              </div>
            </div>
            <div class="pa-3">
              <div class="text-body-medium">{{ formatCapturedAt(shot.capturedAt) }}</div>
              <div class="text-body-small text-on-surface-variant">
                {{ shot.width }}×{{ shot.height }} · {{ formatFileSize(shot.size) }}
              </div>
            </div>
          </v-card>
        </div>
        <v-divider />
        <div class="text-body-small text-on-surface-variant pa-3">
          {{ t('screenshots.count', { count: screenshots.length }) }}
        </div>
      </template>
    </v-card>

    <v-dialog v-model="previewOpen" max-width="1100">
      <v-card v-if="selected" variant="flat" color="surface-container-high">
        <v-card-title class="d-flex align-center ga-3 pa-4">
          <div class="flex-grow-1 min-width-0">
            <div class="text-title-medium">{{ formatCapturedAt(selected.capturedAt) }}</div>
            <div class="text-body-small text-on-surface-variant text-truncate">
              {{ selected.fileName }}
            </div>
          </div>
          <v-btn icon variant="text" :aria-label="t('common.close')" @click="selected = null">
            <v-icon>mdi-close</v-icon>
          </v-btn>
        </v-card-title>
        <v-divider />
        <v-card-text class="pa-4 d-flex justify-center align-center">
          <img
            v-if="!imageFailed(selected.fileName)"
            :src="imageUrl(selected)"
            :alt="selected.fileName"
            class="preview-image"
            @error="markImageFailed(selected.fileName)"
          />
          <div v-else class="text-center pa-8 text-on-surface-variant">
            <v-icon size="40" class="mb-2">mdi-alert-circle-outline</v-icon>
            <div class="text-body-medium">{{ t('screenshots.previewFailed') }}</div>
          </div>
        </v-card-text>
        <v-divider />
        <v-card-actions class="pa-3">
          <v-spacer />
          <v-btn
            color="primary"
            variant="tonal"
            prepend-icon="mdi-content-copy"
            :loading="copying"
            :disabled="copying"
            @click="copySelected"
          >
            {{ t('screenshots.copyImage') }}
          </v-btn>
          <v-btn variant="text" @click="selected = null">{{ t('common.close') }}</v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
.screenshot-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 12px;
}

.screenshot-card {
  cursor: pointer;
  overflow: hidden;
  transition: transform 0.15s ease;
}

.screenshot-card:hover {
  transform: translateY(-1px);
}

.screenshot-card:focus-visible {
  outline: 2px solid rgb(var(--v-theme-primary));
  outline-offset: 2px;
}

.screenshot-image {
  aspect-ratio: 16 / 9;
  background: rgb(var(--v-theme-surface-variant));
}

.screenshot-image img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.screenshot-broken {
  display: flex;
  width: 100%;
  height: 100%;
  align-items: center;
  justify-content: center;
}

.preview-image {
  display: block;
  max-width: 100%;
  max-height: 60vh;
  object-fit: contain;
}

.min-width-0 {
  min-width: 0;
}
</style>
