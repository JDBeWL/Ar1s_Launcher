<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { useSettingsStore } from '../../stores/settings';
import { useNotificationStore } from '../../stores/notificationStore';
import { useEventSubscription } from '../../composables/useEventSubscription';
import { LauncherCommandError } from '../../services/invoke';
import { configApi, javaApi } from '../../services';
import type { JavaDownloadOption, JavaDownloadProgress } from '../../types/events';
import { formatFileSize, getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const settingsStore = useSettingsStore();
const { t } = useI18n();
const javaPath = ref('');
const isJavaPathValid = ref(false);
const loadingJava = ref(false);
const javaVersion = ref('');
const customJavaPaths = ref<string[]>([]);
const inputJavaPath = ref('');

watch(
  javaPath,
  (val) => {
    inputJavaPath.value = val;
  },
  { immediate: true },
);

async function loadJavaPath() {
  try {
    const path = await configApi.loadConfigKey('javaPath');
    javaPath.value = path || '';
    inputJavaPath.value = javaPath.value;
    if (javaPath.value) {
      isJavaPathValid.value = await javaApi.validateJavaPath(javaPath.value);
      if (isJavaPathValid.value) {
        await getJavaVersion();
      }
    }
  } catch (error) {
    logError('Failed to load Java path', error, 'JavaSettings');
  }
}

async function toggleAutoMatchJava(value: boolean | null) {
  settingsStore.autoMatchJava = value === true;
  await settingsStore.saveAutoMatchJava();
}

async function loadCustomJavaPaths() {
  try {
    customJavaPaths.value = await javaApi.getCustomJavaPaths();
    await loadCustomJavaVersions();
  } catch (error) {
    logError('Failed to load custom Java paths', error, 'JavaSettings');
  }
}

async function getJavaVersion() {
  try {
    javaVersion.value = await javaApi.getJavaVersion(javaPath.value);
  } catch {
    javaVersion.value = '';
  }
}

async function findJavaInstallations() {
  try {
    loadingJava.value = true;
    await settingsStore.findJavaInstallations();
    await loadDetectedJavaVersions();

    if (settingsStore.javaInstallations.length > 0 && !javaPath.value) {
      await selectJavaPath(settingsStore.javaInstallations[0]);
    }
  } catch (err) {
    logError('Failed to find Java installations', err, 'JavaSettings');
  } finally {
    loadingJava.value = false;
  }
}

async function selectJavaPath(path: string) {
  try {
    javaPath.value = path;
    inputJavaPath.value = path;
    await configApi.saveConfigKey('javaPath', path);
    isJavaPathValid.value = await javaApi.validateJavaPath(path);
    if (isJavaPathValid.value) {
      await getJavaVersion();
    }
  } catch (err) {
    logError('Failed to set Java path', err, 'JavaSettings');
  }
}

async function browseJavaPath() {
  try {
    const selected = await open({
      multiple: false,
      title: t('settings.java.browseTitle'),
      filters: [
        {
          name: 'Java',
          extensions: ['exe', ''],
        },
      ],
    });
    if (selected) {
      await selectJavaPath(selected as string);
    }
  } catch (err) {
    logError('Failed to browse Java path', err, 'JavaSettings');
  }
}

async function commitInputPath() {
  const path = inputJavaPath.value.trim();
  if (!path) return;
  if (path === javaPath.value) return;
  await selectJavaPath(path);
}

function onInputKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault();
    (e.target as HTMLInputElement).blur();
  }
}

async function addCustomJava() {
  try {
    const selected = await open({
      multiple: false,
      title: t('settings.java.browseTitle'),
      filters: [
        {
          name: 'Java',
          extensions: ['exe', ''],
        },
      ],
    });
    if (selected) {
      const path = selected as string;
      const isValid = await javaApi.validateJavaPath(path);
      if (!isValid) {
        return;
      }
      await javaApi.addCustomJavaPath(path);
      await loadCustomJavaPaths();
    }
  } catch (err) {
    logError('Failed to add custom Java path', err, 'JavaSettings');
  }
}

async function removeCustomJava(path: string) {
  try {
    await javaApi.removeCustomJavaPath(path);
    await loadCustomJavaPaths();
  } catch (err) {
    logError('Failed to remove custom Java path', err, 'JavaSettings');
  }
}

async function fetchJavaVersion(path: string): Promise<string> {
  try {
    return await javaApi.getJavaVersion(path);
  } catch {
    return '';
  }
}

const customJavaVersions = ref<Record<string, string>>({});

async function loadCustomJavaVersions() {
  const entries = await Promise.all(
    customJavaPaths.value.map(async (path) => {
      const version = await fetchJavaVersion(path);
      return [path, version] as const;
    }),
  );
  customJavaVersions.value = Object.fromEntries(entries);
}

const detectedJavaVersions = ref<Record<string, string>>({});

async function loadDetectedJavaVersions() {
  const entries = await Promise.all(
    settingsStore.javaInstallations.map(async (path) => {
      const version = await fetchJavaVersion(path);
      return [path, version] as const;
    }),
  );
  detectedJavaVersions.value = Object.fromEntries(entries);
}

const notificationStore = useNotificationStore();
const downloadOptions = ref<JavaDownloadOption[]>([]);
const catalogLoading = ref(false);
const catalogError = ref('');
const activeDownload = ref<JavaDownloadProgress | null>(null);
const downloadFeature = ref<number | null>(null);
const deleteDialog = ref(false);
const managedToDelete = ref<JavaDownloadOption | null>(null);
const deleting = ref(false);

function normalizePath(path: string) {
  return path.replace(/\\/g, '/').toLowerCase();
}

const managedPaths = computed(() => {
  const paths = downloadOptions.value
    .map((option) => option.installedPath)
    .filter((path): path is string => Boolean(path))
    .map(normalizePath);
  return new Set(paths);
});

function isManagedJava(path: string) {
  return managedPaths.value.has(normalizePath(path));
}

function formatSpeed(kbps: number) {
  if (!Number.isFinite(kbps) || kbps <= 0) return '0 KB/s';
  return kbps >= 1024 ? `${(kbps / 1024).toFixed(1)} MB/s` : `${Math.round(kbps)} KB/s`;
}

async function loadDownloadOptions() {
  catalogLoading.value = true;
  catalogError.value = '';
  try {
    downloadOptions.value = await javaApi.getJavaDownloadOptions();
  } catch (error) {
    logError('Failed to load Java download options', error, 'JavaSettings');
    catalogError.value = getErrorMessage(error);
  } finally {
    catalogLoading.value = false;
  }
}

async function refreshJavaState() {
  await javaApi.refreshJavaInstallations();
  await settingsStore.findJavaInstallations();
  await Promise.all([loadDetectedJavaVersions(), loadDownloadOptions(), loadJavaPath()]);
}

async function afterJavaChanged(autoSelect: boolean, feature?: number) {
  try {
    await refreshJavaState();
  } catch (error) {
    logError('Failed to refresh Java state', error, 'JavaSettings');
  }
  if (!autoSelect || javaPath.value || feature == null) return;

  const installed = downloadOptions.value.find((option) => option.feature === feature);
  if (installed?.installedPath) {
    await selectJavaPath(installed.installedPath);
  }
}

function onDownloadProgress(event: { payload: JavaDownloadProgress }) {
  const progress = event.payload;
  activeDownload.value = progress;

  if (progress.phase === 'downloading' || progress.phase === 'extracting') {
    downloadFeature.value = progress.feature;
    return;
  }

  downloadFeature.value = null;
  activeDownload.value = null;

  if (progress.phase === 'completed') {
    notificationStore.success(
      t('settings.java.downloaded'),
      t('settings.java.downloadedHint', { version: `Java ${progress.feature}` }),
    );
    void afterJavaChanged(true, progress.feature);
  } else if (progress.phase === 'cancelled') {
    notificationStore.info(t('settings.java.downloadCancelled'), t('settings.java.downloadCancelledHint'));
  }
}

const downloadProgressSub = useEventSubscription<JavaDownloadProgress>('java-download-progress', onDownloadProgress);

async function downloadJava(option: JavaDownloadOption) {
  if (downloadFeature.value !== null) return;

  downloadFeature.value = option.feature;
  activeDownload.value = {
    feature: option.feature,
    phase: 'downloading',
    bytesDownloaded: 0,
    totalBytes: option.size,
    speed: 0,
    percent: 0,
  };

  try {
    await javaApi.downloadJavaRuntime(option.feature);
  } catch (error) {
    if (!(error instanceof LauncherCommandError && error.cancelled)) {
      logError('Failed to download Java', error, 'JavaSettings');
      notificationStore.error(t('settings.java.downloadFailed'), getErrorMessage(error));
    }
    downloadFeature.value = null;
    activeDownload.value = null;
  }
}

async function cancelJavaDownload() {
  try {
    await javaApi.cancelJavaDownload();
  } catch (error) {
    logError('Failed to cancel Java download', error, 'JavaSettings');
  }
}

function askDeleteManaged(option: JavaDownloadOption) {
  managedToDelete.value = option;
  deleteDialog.value = true;
}

function deleteAffectsSelection() {
  if (!managedToDelete.value?.installedPath || !javaPath.value) return false;
  const root = managedToDelete.value.installedPath.replace(/\/bin\/java(\.exe)?$/i, '');
  const selected = normalizePath(javaPath.value);
  const normalizedRoot = normalizePath(root);
  return selected === normalizedRoot || selected.startsWith(`${normalizedRoot}/`);
}

async function confirmDeleteManaged() {
  if (!managedToDelete.value || deleting.value) return;

  deleting.value = true;
  const feature = managedToDelete.value.feature;
  try {
    await javaApi.deleteJavaRuntime(feature);
    deleteDialog.value = false;
    managedToDelete.value = null;
    await afterJavaChanged(false);
    notificationStore.success(
      t('settings.java.deleted'),
      t('settings.java.deletedHint', { version: `Java ${feature}` }),
    );
  } catch (error) {
    logError('Failed to delete Java', error, 'JavaSettings');
    notificationStore.error(t('settings.java.deleteFailed'), getErrorMessage(error));
  } finally {
    deleting.value = false;
  }
}

onMounted(async () => {
  await downloadProgressSub.subscribe();
  await Promise.all([loadJavaPath(), loadCustomJavaPaths(), settingsStore.loadSettings(), loadDownloadOptions()]);

  const current = await javaApi.getJavaDownloadState();
  if (current) {
    activeDownload.value = current;
    downloadFeature.value = current.feature;
  }

  if (!settingsStore.hasFoundJavaInstallations && settingsStore.javaInstallations.length === 0) {
    await findJavaInstallations();
  } else {
    await loadDetectedJavaVersions();
  }
});
</script>

<template>
  <div class="settings-group">
    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-4">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-file-cog-outline</v-icon>
            <span class="text-title-medium">{{ t('settings.java.pathTitle') }}</span>
          </div>
          <div class="d-flex ga-2">
            <v-btn variant="tonal" color="primary" size="small" :loading="loadingJava" @click="findJavaInstallations">
              <v-icon start size="18">mdi-magnify</v-icon>
              {{ t('settings.java.autoFind') }}
            </v-btn>
            <v-btn variant="tonal" color="secondary" size="small" @click="browseJavaPath">
              <v-icon start size="18">mdi-folder-open-outline</v-icon>
              {{ t('common.browse') }}
            </v-btn>
          </div>
        </div>

        <v-text-field
          v-model="inputJavaPath"
          density="compact"
          variant="outlined"
          :label="t('settings.java.pathLabel')"
          :placeholder="t('settings.java.pathPlaceholder')"
          hide-details
          class="mb-4 font-mono-input"
          @keydown="onInputKeydown"
          @blur="commitInputPath"
        >
          <template #prepend-inner>
            <v-icon size="18" color="on-surface-variant">mdi-console</v-icon>
          </template>
          <template #append-inner>
            <v-icon
              v-if="inputJavaPath && inputJavaPath === javaPath"
              :color="isJavaPathValid ? 'success' : 'error'"
              size="18"
            >
              {{ isJavaPathValid ? 'mdi-check-circle' : 'mdi-alert-circle' }}
            </v-icon>
          </template>
        </v-text-field>

        <div v-if="javaPath" class="d-flex align-center mb-4">
          <v-icon size="16" :color="isJavaPathValid ? 'success' : 'error'" class="mr-2">
            {{ isJavaPathValid ? 'mdi-check-circle' : 'mdi-alert-circle' }}
          </v-icon>
          <span v-if="isJavaPathValid" class="text-body-small text-on-surface-variant">
            {{ javaVersion || t('settings.java.pathValid') }}
          </span>
          <span v-else class="text-body-small text-error">{{ t('settings.java.pathInvalid') }}</span>
        </div>

        <div v-if="settingsStore.javaInstallations.length > 0" class="mb-2">
          <div class="text-body-medium text-on-surface-variant mb-2">{{ t('settings.java.detected') }}</div>
          <v-list density="compact" lines="two" class="java-list" bg-color="surface-container-high">
            <v-list-item
              v-for="path in settingsStore.javaInstallations"
              :key="'detected-' + path"
              :active="javaPath === path"
              @click="selectJavaPath(path)"
            >
              <template #prepend>
                <v-icon :color="javaPath === path ? 'primary' : 'on-surface-variant'">
                  {{ javaPath === path ? 'mdi-radiobox-marked' : 'mdi-radiobox-blank' }}
                </v-icon>
              </template>
              <v-list-item-title class="text-body-medium font-mono">
                {{ path.replace(/\\/g, '/') }}
              </v-list-item-title>
              <v-list-item-subtitle v-if="detectedJavaVersions[path]" class="text-body-small">
                {{ detectedJavaVersions[path] }}
              </v-list-item-subtitle>
              <template #append>
                <v-chip v-if="isManagedJava(path)" size="x-small" color="primary" variant="tonal">
                  {{ t('settings.java.managedChip') }}
                </v-chip>
              </template>
            </v-list-item>
          </v-list>
        </div>

        <v-alert v-if="!javaPath" color="warning-container" density="compact">
          <template #prepend>
            <v-icon color="on-warning-container">mdi-alert-outline</v-icon>
          </template>
          <span class="text-on-warning-container">{{ t('settings.java.noneDetected') }}</span>
        </v-alert>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-3">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-download</v-icon>
          <div>
            <span class="text-title-medium">{{ t('settings.java.downloadTitle') }}</span>
            <div class="text-body-small text-on-surface-variant">
              {{ t('settings.java.downloadHint') }}
            </div>
          </div>
        </div>

        <div v-if="activeDownload" class="download-active mb-3">
          <div class="d-flex align-center ga-2 mb-2">
            <span class="text-body-medium">
              {{
                activeDownload.phase === 'extracting'
                  ? t('settings.java.extracting', { version: `Java ${activeDownload.feature}` })
                  : t('settings.java.downloading', { version: `Java ${activeDownload.feature}` })
              }}
            </span>
            <v-spacer />
            <span v-if="activeDownload.phase === 'downloading'" class="text-body-small text-on-surface-variant">
              {{ activeDownload.percent }}% · {{ formatSpeed(activeDownload.speed) }}
            </span>
            <v-btn
              v-if="activeDownload.phase === 'downloading'"
              size="small"
              variant="text"
              @click="cancelJavaDownload"
            >
              {{ t('common.cancel') }}
            </v-btn>
          </div>
          <v-progress-linear
            v-if="activeDownload.phase === 'downloading'"
            :model-value="activeDownload.percent"
            height="6"
            rounded
            color="primary"
          />
          <v-progress-linear v-else indeterminate height="6" rounded color="primary" />
        </div>

        <div v-if="catalogLoading && downloadOptions.length === 0" class="d-flex justify-center pa-6">
          <v-progress-circular indeterminate size="24" color="primary" />
        </div>

        <div v-else-if="catalogError" class="text-center pa-4">
          <div class="text-body-medium text-error mb-3">{{ catalogError }}</div>
          <v-btn size="small" variant="tonal" color="primary" @click="loadDownloadOptions">
            {{ t('common.retry') }}
          </v-btn>
        </div>

        <v-list v-else density="compact" lines="two" bg-color="surface-container-high">
          <v-list-item v-for="option in downloadOptions" :key="option.feature">
            <v-list-item-title class="d-flex align-center ga-2">
              <span class="text-body-medium">Java {{ option.feature }} · LTS</span>
              <v-chip v-if="option.state === 'installed'" size="x-small" color="success" variant="tonal">
                {{ t('settings.java.installed') }}
              </v-chip>
              <v-chip v-else-if="option.state === 'broken'" size="x-small" color="warning" variant="tonal">
                {{ t('settings.java.broken') }}
              </v-chip>
            </v-list-item-title>
            <v-list-item-subtitle class="text-body-small">
              {{ option.releaseName }} · {{ formatFileSize(option.size) }}
              <template v-if="option.installedVersion"> · {{ option.installedVersion }}</template>
            </v-list-item-subtitle>
            <template #append>
              <v-btn
                v-if="option.state === 'notInstalled'"
                size="small"
                variant="tonal"
                color="primary"
                :disabled="downloadFeature !== null"
                :aria-label="t('settings.java.downloadAria', { version: `Java ${option.feature}` })"
                @click="downloadJava(option)"
              >
                {{
                  downloadFeature === option.feature ? t('settings.java.downloadingShort') : t('settings.java.download')
                }}
              </v-btn>
              <v-btn
                v-else
                icon
                size="small"
                variant="text"
                color="error"
                :aria-label="t('settings.java.deleteAria', { version: `Java ${option.feature}` })"
                :disabled="downloadFeature !== null"
                @click="askDeleteManaged(option)"
              >
                <v-icon size="18">mdi-delete-outline</v-icon>
              </v-btn>
            </template>
          </v-list-item>
        </v-list>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-4">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-bookmark-multiple-outline</v-icon>
            <div>
              <span class="text-title-medium">{{ t('settings.java.customTitle') }}</span>
              <div class="text-body-small text-on-surface-variant">{{ t('settings.java.customHint') }}</div>
            </div>
          </div>
          <v-btn variant="tonal" color="primary" size="small" @click="addCustomJava">
            <v-icon start size="18">mdi-plus</v-icon>
            {{ t('common.add') }}
          </v-btn>
        </div>

        <div v-if="customJavaPaths.length > 0">
          <v-list density="compact" lines="two" class="java-list" bg-color="surface-container-high">
            <v-list-item
              v-for="path in customJavaPaths"
              :key="'custom-' + path"
              :active="javaPath === path"
              @click="selectJavaPath(path)"
            >
              <template #prepend>
                <v-icon :color="javaPath === path ? 'primary' : 'on-surface-variant'">
                  {{ javaPath === path ? 'mdi-radiobox-marked' : 'mdi-radiobox-blank' }}
                </v-icon>
              </template>
              <v-list-item-title class="text-body-medium font-mono">
                {{ path.replace(/\\/g, '/') }}
              </v-list-item-title>
              <v-list-item-subtitle v-if="customJavaVersions[path]" class="text-body-small">
                {{ customJavaVersions[path] }}
              </v-list-item-subtitle>
              <template #append>
                <v-btn icon size="x-small" variant="text" color="error" @click.stop="removeCustomJava(path)">
                  <v-icon size="16">mdi-close</v-icon>
                </v-btn>
              </template>
            </v-list-item>
          </v-list>
        </div>
        <div v-else class="text-body-small text-on-surface-variant">
          {{ t('settings.java.customEmpty') }}
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-auto-fix</v-icon>
            <div>
              <span class="text-title-medium">{{ t('settings.java.autoMatchTitle') }}</span>
              <div class="text-body-small text-on-surface-variant">
                {{ t('settings.java.autoMatchHint') }}
              </div>
            </div>
          </div>
          <v-switch
            :model-value="settingsStore.autoMatchJava"
            :aria-label="t('settings.java.autoMatchTitle')"
            color="primary"
            hide-details
            inset
            density="compact"
            @update:model-value="toggleAutoMatchJava"
          />
        </div>
        <v-alert v-if="settingsStore.autoMatchJava" color="secondary-container" density="compact" class="mt-3">
          <template #prepend>
            <v-icon color="on-secondary-container" size="18">mdi-information-outline</v-icon>
          </template>
          <span class="text-on-secondary-container text-body-medium">
            {{ t('settings.java.autoMatchDetail') }}
          </span>
        </v-alert>
      </v-card-text>
    </v-card>

    <v-alert color="secondary-container" class="tip-alert">
      <template #prepend>
        <v-icon size="20" color="on-secondary-container">mdi-information-outline</v-icon>
      </template>
      <template #title>
        <span class="text-body-medium font-weight-medium text-on-secondary-container">
          {{ t('settings.java.tipsTitle') }}
        </span>
      </template>
      <ul class="text-body-small pl-4 mb-0 mt-1 text-on-secondary-container">
        <li>{{ t('settings.java.tip1') }}</li>
        <li>{{ t('settings.java.tip2') }}</li>
        <li>{{ t('settings.java.tip3') }}</li>
        <li>{{ t('settings.java.tip4') }}</li>
        <li>{{ t('settings.java.tip5') }} <code class="font-mono">./runtime/java/bin/java.exe</code></li>
      </ul>
    </v-alert>

    <v-dialog v-model="deleteDialog" max-width="420">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-title-large font-weight-bold mb-2">{{ t('settings.java.deleteTitle') }}</div>
          <div class="text-body-medium text-on-surface-variant">
            {{ t('settings.java.deleteConfirmPrefix') }}
            <strong class="text-on-surface">Java {{ managedToDelete?.feature }}</strong>
            {{ t('settings.java.deleteConfirmSuffix') }}
          </div>
          <div v-if="managedToDelete?.installedPath" class="text-body-small text-on-surface-variant mt-2 font-mono">
            {{ managedToDelete.installedPath }}
          </div>
          <div v-if="deleteAffectsSelection()" class="text-body-small text-warning mt-3">
            {{ t('settings.java.deleteSelectedWarning') }}
          </div>
          <div class="text-body-small text-on-surface-variant mt-2">
            {{ t('settings.java.deleteInstanceWarning') }}
          </div>
        </v-card-text>
        <v-card-actions class="pa-4 pt-0">
          <v-spacer />
          <v-btn variant="text" @click="deleteDialog = false">{{ t('common.cancel') }}</v-btn>
          <v-btn color="error" :loading="deleting" @click="confirmDeleteManaged">
            {{ t('common.delete') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
.java-list {
  max-height: 200px;
  overflow-y: auto;
}

.download-active {
  padding: 12px;
  border-radius: 8px;
  background: rgb(var(--v-theme-surface-container-high));
}

.font-mono {
  font-family: 'Consolas', 'Monaco', monospace;
  font-size: 0.875rem;
}

.font-mono-input :deep(input) {
  font-family: 'Consolas', 'Monaco', monospace;
  font-size: 0.875rem;
}
</style>
