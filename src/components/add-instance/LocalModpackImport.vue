<script setup lang="ts">
/**
 * 从本地压缩包导入整合包（.mrpack 与 CurseForge zip 自动识别）
 * 选文件后先让后端读清单展示包信息，再由安装页接管具体的下载进度
 */
import { computed, ref } from 'vue';
import { useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { modpackApi } from '../../services';
import { useNotificationStore } from '../../stores/notificationStore';
import { formatFileSize, getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';
import type { LocalModpackPreview } from '../../types/events';

const router = useRouter();
const notificationStore = useNotificationStore();
const { t } = useI18n();

const preview = ref<LocalModpackPreview | null>(null);
const reading = ref(false);
const readError = ref('');

async function pickFile() {
  if (reading.value) return;

  try {
    const selected = await open({
      multiple: false,
      directory: false,
      title: t('localModpack.pickTitle'),
      filters: [{ name: t('localModpack.pickFilter'), extensions: ['mrpack', 'zip'] }],
    });
    if (!selected) return;

    await loadPreview(Array.isArray(selected) ? selected[0] : selected);
  } catch (e) {
    logError('Failed to pick a modpack file', e, 'LocalModpackImport');
    notificationStore.error(t('localModpack.readFailed'), getErrorMessage(e));
  }
}

async function loadPreview(path: string) {
  reading.value = true;
  readError.value = '';
  preview.value = null;

  try {
    preview.value = await modpackApi.previewLocalModpack(path);
  } catch (e) {
    logError('Failed to read local modpack', e, 'LocalModpackImport');
    readError.value = getErrorMessage(e);
  } finally {
    reading.value = false;
  }
}

function startInstall() {
  if (!preview.value) return;

  router.push({
    path: '/install-modpack',
    query: {
      file: preview.value.path,
      title: preview.value.name,
    },
  });
}

const fileName = computed(() => {
  const path = preview.value?.path;
  if (!path) return '';
  const index = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return index === -1 ? path : path.slice(index + 1);
});

const loaderLabel = computed(() => {
  const loader = preview.value?.loader;
  if (!loader) return t('localModpack.vanilla');
  return preview.value?.loaderVersion ? `${loader} ${preview.value.loaderVersion}` : loader;
});
</script>

<template>
  <v-card variant="flat" color="surface-container">
    <v-card-text class="pa-4">
      <div class="text-title-small text-on-surface-variant mb-3">
        <v-icon size="18" class="mr-1">mdi-file-import-outline</v-icon>
        {{ t('localModpack.title') }}
      </div>

      <div class="text-body-small text-on-surface-variant mb-4">{{ t('localModpack.hint') }}</div>

      <div class="d-flex align-center ga-2 flex-wrap">
        <v-btn
          color="primary"
          variant="tonal"
          prepend-icon="mdi-folder-open-outline"
          :loading="reading"
          @click="pickFile"
        >
          {{ preview ? t('localModpack.changeFile') : t('localModpack.pickFile') }}
        </v-btn>
        <v-chip v-if="preview" size="small" variant="tonal" :text="fileName" class="text-truncate" />
        <span v-else-if="!reading" class="text-body-small text-on-surface-variant">{{ t('localModpack.noFile') }}</span>
      </div>

      <div v-if="reading" class="d-flex align-center mt-4 text-body-small text-on-surface-variant">
        <v-progress-circular indeterminate size="14" width="2" class="mr-2" />
        {{ t('localModpack.reading') }}
      </div>

      <v-alert v-else-if="readError" type="error" variant="tonal" density="comfortable" class="mt-4">
        <template #title>{{ t('localModpack.readFailed') }}</template>
        <div>{{ readError }}</div>
        <div class="text-body-small mt-1">{{ t('localModpack.readFailedHint') }}</div>
      </v-alert>

      <template v-else-if="preview">
        <v-divider class="my-4" />

        <div class="text-title-small mb-3">{{ preview.name }}</div>
        <div v-if="preview.summary" class="text-body-small text-on-surface-variant mb-3">{{ preview.summary }}</div>

        <v-row density="compact">
          <v-col cols="6" sm="3">
            <div class="text-body-small text-on-surface-variant">{{ t('localModpack.minecraftVersion') }}</div>
            <div class="text-body-medium">{{ preview.minecraft }}</div>
          </v-col>
          <v-col cols="6" sm="3">
            <div class="text-body-small text-on-surface-variant">{{ t('localModpack.loaderLabel') }}</div>
            <div class="text-body-medium">{{ loaderLabel }}</div>
          </v-col>
          <v-col cols="6" sm="3">
            <div class="text-body-small text-on-surface-variant">{{ t('localModpack.fileCount') }}</div>
            <div class="text-body-medium">{{ preview.fileCount }}</div>
          </v-col>
          <v-col cols="6" sm="3">
            <div class="text-body-small text-on-surface-variant">{{ t('localModpack.totalSize') }}</div>
            <div class="text-body-medium">{{ formatFileSize(preview.totalSize) }}</div>
          </v-col>
        </v-row>

        <div class="d-flex justify-end mt-4">
          <v-btn color="primary" prepend-icon="mdi-download" @click="startInstall">
            {{ t('localModpack.install') }}
          </v-btn>
        </div>
      </template>
    </v-card-text>
  </v-card>
</template>
