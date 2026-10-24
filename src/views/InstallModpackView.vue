<template>
  <v-container fluid class="page-container pa-4">
    <v-card variant="flat" color="surface-container">
      <v-card-title class="d-flex align-center pa-4">
        <v-btn variant="text" icon="mdi-arrow-left" :disabled="installing" @click="goBack" class="mr-2" />
        <div class="flex-grow-1">
          <div class="d-flex align-center">
            <span class="text-title-large">{{ title }}</span>
            <v-chip v-if="installing" color="primary" variant="tonal" size="small" class="ml-2">
              {{ t('installModpack.installing') }}
            </v-chip>
          </div>
          <div class="text-body-small text-on-surface-variant">
            <template v-if="isLocal">{{ t('localModpack.title') }}</template>
            <template v-else>{{ sourceLabel }} · {{ projectId }}</template>
          </div>
        </div>
      </v-card-title>

      <v-divider />

      <v-card-text class="pa-4">
        <v-expand-transition>
          <v-card v-if="installing" color="surface-container-high" class="mb-4" variant="flat">
            <v-card-text class="pa-4">
              <div class="d-flex align-center justify-space-between mb-2">
                <div class="d-flex align-center flex-grow-1 overflow-hidden">
                  <v-progress-circular
                    v-if="installProgress.indeterminate"
                    indeterminate
                    size="18"
                    width="2"
                    color="primary"
                    class="mr-2 flex-shrink-0"
                  />
                  <v-icon v-else size="18" color="primary" class="mr-2 flex-shrink-0">mdi-download</v-icon>
                  <span class="text-body-medium text-truncate">{{ installProgress.message }}</span>
                </div>
                <span
                  v-if="!validatingFiles && !completingFiles"
                  class="text-body-medium font-weight-medium text-primary num-mono ml-4"
                >
                  {{ installProgress.progress }}%
                </span>
              </div>
              <v-progress-linear
                :model-value="installProgress.progress"
                :indeterminate="installProgress.indeterminate"
                height="6"
                rounded
                color="primary"
                bg-color="surface-container"
              />

              <!-- CurseForge 受限文件：安装本体已完成，这些要用户手动下载进 mods -->
              <v-alert
                v-if="manualDownloads.length > 0 && !validatingFiles && !postInstallChecked"
                type="warning"
                variant="tonal"
                density="comfortable"
                class="mt-3"
              >
                <template #title>
                  {{ t('installModpack.manualDownloadsTitle', { count: manualDownloads.length }, manualDownloads.length) }}
                </template>
                <div class="text-body-small">{{ t('installModpack.manualDownloadsHint') }}</div>
                <div class="mt-2">
                  <div
                    v-for="file in manualDownloads"
                    :key="file.url || file.name"
                    class="d-flex align-center ga-2 manual-file"
                  >
                    <v-icon size="14" class="flex-shrink-0">mdi-file-alert-outline</v-icon>
                    <span class="text-body-small text-truncate" :title="file.name">{{ file.name }}</span>
                  </div>
                </div>
                <div class="d-flex justify-end ga-2 mt-3">
                  <v-btn
                    v-if="manualDownloads.some((file) => file.url)"
                    size="small"
                    variant="text"
                    prepend-icon="mdi-content-copy"
                    @click="copyManualUrls"
                  >
                    {{ t('installModpack.copyManualUrls') }}
                  </v-btn>
                  <v-btn
                    size="small"
                    variant="tonal"
                    color="primary"
                    @click="runPostInstall"
                  >
                    {{ t('installModpack.continueInstall') }}
                  </v-btn>
                </div>
              </v-alert>

              <div v-if="validatingFiles" class="d-flex align-center mt-3 text-body-small text-on-surface-variant">
                <v-progress-circular indeterminate size="14" width="2" class="mr-2" />
                {{ t('installModpack.validating') }}
              </div>

              <template v-else-if="postInstallChecked && missingFileCount > 0">
                <v-alert v-if="!completionError" type="info" variant="tonal" density="comfortable" class="mt-3">
                  <template #title>{{ t('installModpack.completingTitle') }}</template>
                  {{ t('installModpack.completingHint', { count: missingFileCount }, missingFileCount) }}
                </v-alert>
                <v-alert v-else type="warning" variant="tonal" density="comfortable" class="mt-3">
                  <template #title>{{ t('installModpack.completingFailedTitle') }}</template>
                  {{ t('installModpack.completingFailedHint', { count: missingFileCount }, missingFileCount) }}
                </v-alert>

                <div v-if="completingFiles" class="mt-3">
                  <div class="d-flex justify-space-between text-body-small text-on-surface-variant num-mono">
                    <span v-if="versionDownload">
                      {{ formatBytes(versionDownload.bytes_downloaded) }} /
                      {{ formatBytes(versionDownload.total_bytes) }}
                    </span>
                    <span v-else>{{ t('installModpack.preparingCompletion') }}</span>
                    <span v-if="versionDownload">
                      {{ formatSpeed(versionDownload.speed) }} · {{ versionDownload.percent }}%
                    </span>
                  </div>
                  <v-progress-linear
                    :model-value="versionDownload?.percent ?? 0"
                    :indeterminate="!versionDownload"
                    height="4"
                    rounded
                    color="primary"
                    bg-color="surface-container"
                    class="mt-1"
                  />
                </div>
                <div v-if="completionError" class="text-body-small text-error mt-2">{{ completionError }}</div>
              </template>

              <template v-else-if="filesStage">
                <div class="d-flex flex-wrap align-center ga-2 mt-3">
                  <v-chip size="x-small" color="primary" variant="tonal" prepend-icon="mdi-download">
                    {{ t('installModpack.downloadingCount', { count: activeFiles.length }) }}
                  </v-chip>
                  <v-chip v-if="queuedFileCount > 0" size="x-small" variant="tonal">
                    {{ t('installModpack.queuedCount', { count: queuedFileCount }) }}
                  </v-chip>
                  <v-chip v-if="fileProgress" size="x-small" color="success" variant="tonal">
                    {{
                      t('installModpack.completedCount', {
                        completed: fileProgress.completed_files,
                        total: fileProgress.total_files,
                      })
                    }}
                  </v-chip>
                  <v-spacer />
                  <span
                    v-if="fileProgress && fileProgress.speed > 0"
                    class="text-body-small text-on-surface-variant num-mono"
                  >
                    {{ formatSpeed(fileProgress.speed) }}
                  </span>
                </div>

                <div
                  v-if="fileProgress && fileProgress.total_bytes > 0"
                  class="d-flex justify-space-between text-body-small text-on-surface-variant num-mono mt-1"
                >
                  <span>
                    {{ formatBytes(fileProgress.downloaded_bytes) }} /
                    {{ formatBytes(fileProgress.total_bytes) }}
                  </span>
                  <span v-if="etaText">{{ etaText }}</span>
                </div>

                <div class="file-list mt-2">
                  <transition-group name="file-row" tag="div">
                    <div v-for="file in activeFiles" :key="file.path" class="file-row">
                      <div class="d-flex align-center ga-2">
                        <v-icon size="15" class="text-on-surface-variant flex-shrink-0">
                          mdi-file-download-outline
                        </v-icon>
                        <span class="file-name text-body-small" :title="file.path">{{ baseName(file.path) }}</span>
                        <v-spacer />
                        <span class="text-body-small text-on-surface-variant num-mono flex-shrink-0">
                          {{ fileSizeLabel(file) }}
                        </span>
                      </div>
                      <div v-if="dirName(file.path)" class="file-dir text-body-small text-on-surface-variant">
                        {{ dirName(file.path) }}
                      </div>
                      <v-progress-linear
                        class="mt-1"
                        :model-value="filePercent(file)"
                        :indeterminate="file.total === 0"
                        height="3"
                        rounded
                        color="primary"
                        bg-color="surface-container"
                      />
                    </div>
                  </transition-group>

                  <div
                    v-if="activeFiles.length === 0"
                    class="text-body-small text-on-surface-variant d-flex align-center pa-2"
                  >
                    <v-progress-circular indeterminate size="14" width="2" class="mr-2" />
                    {{ t('installModpack.schedulingNext') }}
                  </div>
                </div>
              </template>

              <!-- 核心文件与模组并行下载，所以这里不能是 else-if：两段同时进行时要一起显示 -->
              <div v-if="gameStage && versionDownload" class="mt-3">
                <div class="d-flex justify-space-between text-body-small text-on-surface-variant num-mono">
                  <span>
                    {{ formatBytes(versionDownload.bytes_downloaded) }} /
                    {{ formatBytes(versionDownload.total_bytes) }}
                  </span>
                  <span>{{ formatSpeed(versionDownload.speed) }} · {{ versionDownload.percent }}%</span>
                </div>
                <v-progress-linear
                  :model-value="versionDownload.percent"
                  height="4"
                  rounded
                  color="primary"
                  bg-color="surface-container"
                  class="mt-1"
                />
                <div v-if="versionDownloadDone" class="text-body-small text-on-surface-variant mt-2">
                  {{ t('installModpack.gameReady') }}
                </div>
              </div>

              <div v-if="!validatingFiles" class="d-flex justify-end ga-2 mt-3">
                <template v-if="postInstallChecked && missingFileCount > 0">
                  <v-btn
                    v-if="completingFiles"
                    variant="text"
                    color="error"
                    size="small"
                    :loading="cancelling"
                    :disabled="cancelling"
                    @click="stopCompletion"
                  >
                    <v-icon start size="16">mdi-close</v-icon>
                    {{ t('installModpack.stopCompletion') }}
                  </v-btn>
                  <template v-else-if="completionError">
                    <v-btn variant="text" @click="finishLater">{{ t('installModpack.finishLater') }}</v-btn>
                    <v-btn color="primary" variant="flat" @click="completeMissingFiles">
                      {{ t('installModpack.retryCompletion') }}
                    </v-btn>
                  </template>
                </template>
                <v-btn
                  v-else
                  variant="text"
                  color="error"
                  size="small"
                  :loading="cancelling"
                  :disabled="cancelling"
                  @click="cancelInstall"
                >
                  <v-icon start size="16">mdi-close</v-icon>
                  {{ t('installModpack.cancel') }}
                </v-btn>
              </div>
            </v-card-text>
          </v-card>
        </v-expand-transition>

        <template v-if="loadingVersions">
          <v-skeleton-loader type="text" class="mb-4" />
          <v-row>
            <v-col cols="12" md="6">
              <v-skeleton-loader type="text" />
            </v-col>
            <v-col cols="12" md="6">
              <v-skeleton-loader type="text" />
            </v-col>
          </v-row>
        </template>

        <template v-else-if="!installing && isLocal">
          <v-alert v-if="localError" type="error" variant="tonal" class="mb-4">
            <template #title>{{ t('localModpack.readFailed') }}</template>
            {{ localError }}
            <div class="text-body-small mt-1">{{ t('localModpack.readFailedHint') }}</div>
          </v-alert>

          <template v-else-if="localPreview">
            <div class="config-section mb-4">
              <div class="text-title-small text-on-surface-variant mb-3">
                <v-icon size="18" class="mr-1">mdi-package-variant-closed</v-icon>
                {{ t('localModpack.contentSection') }}
              </div>

              <v-row density="compact">
                <v-col cols="6" sm="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('localModpack.minecraftVersion') }}</div>
                  <div class="text-body-medium">{{ localPreview.minecraft }}</div>
                </v-col>
                <v-col cols="6" sm="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('localModpack.loaderLabel') }}</div>
                  <div class="text-body-medium">{{ localLoaderLabel }}</div>
                </v-col>
                <v-col cols="6" sm="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('localModpack.fileCount') }}</div>
                  <div class="text-body-medium">{{ localPreview.fileCount }}</div>
                </v-col>
                <v-col cols="6" sm="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('localModpack.totalSize') }}</div>
                  <div class="text-body-medium">{{ formatBytes(localPreview.totalSize) }}</div>
                </v-col>
              </v-row>
            </div>

            <div class="config-section mb-4">
              <div class="text-title-small text-on-surface-variant mb-3">
                <v-icon size="18" class="mr-1">mdi-rename</v-icon>
                {{ t('localModpack.instanceSection') }}
              </div>

              <v-text-field
                v-model="instanceName"
                :label="t('localModpack.instanceNameLabel')"
                :placeholder="effectiveInstanceName"
                density="comfortable"
                variant="outlined"
                :error-messages="instanceNameError || undefined"
                :loading="checkingInstanceName"
                hide-details="auto"
                counter="64"
                maxlength="64"
              >
                <template #prepend-inner>
                  <v-icon size="18" color="on-surface-variant">mdi-rename</v-icon>
                </template>
                <template #append-inner>
                  <v-fade-transition>
                    <v-icon
                      v-if="instanceName && !instanceNameError && !checkingInstanceName"
                      size="18"
                      color="success"
                    >
                      mdi-check-circle
                    </v-icon>
                  </v-fade-transition>
                </template>
              </v-text-field>

              <div class="text-body-small text-on-surface-variant mt-2">{{ t('localModpack.instanceNameHint') }}</div>
            </div>
          </template>
        </template>

        <template v-else-if="!installing">
          <v-alert v-if="modpackVersions.length === 0" type="warning" variant="tonal" class="mb-4">
            <template #title>{{ t('installModpack.noVersionsTitle') }}</template>
            {{ t('installModpack.noVersionsHint') }}
          </v-alert>

          <template v-else>
            <div class="config-section mb-4">
              <div class="text-title-small text-on-surface-variant mb-3">
                <v-icon size="18" class="mr-1">mdi-tune</v-icon>
                {{ t('installModpack.versionConfig') }}
              </div>

              <v-row density="compact">
                <v-col cols="6" sm="4" md="3">
                  <v-select
                    v-model="selectedLoader"
                    :items="loaderOptions"
                    :label="t('installModpack.loaderLabel')"
                    density="comfortable"
                    variant="outlined"
                    hide-details
                    @update:model-value="onLoaderChange"
                  >
                    <template #prepend-inner>
                      <v-icon size="18" color="on-surface-variant">mdi-puzzle</v-icon>
                    </template>
                  </v-select>
                </v-col>

                <v-col cols="6" sm="4" md="3">
                  <v-select
                    v-model="selectedGameVersion"
                    :items="filteredGameVersions"
                    :label="t('installModpack.gameVersionLabel')"
                    density="comfortable"
                    variant="outlined"
                    :disabled="!selectedLoader"
                    clearable
                    hide-details
                    @update:model-value="onGameVersionChange"
                  >
                    <template #prepend-inner>
                      <v-icon size="18" color="on-surface-variant">mdi-minecraft</v-icon>
                    </template>
                  </v-select>
                </v-col>

                <v-col cols="12" sm="4" md="6">
                  <v-select
                    v-model="selectedVersionId"
                    :items="filteredModpackVersions"
                    item-title="displayName"
                    item-value="id"
                    :label="t('installModpack.modpackVersionLabel')"
                    density="comfortable"
                    variant="outlined"
                    :disabled="!selectedLoader"
                    hide-details
                  >
                    <template #prepend-inner>
                      <v-icon size="18" color="on-surface-variant">mdi-package-variant</v-icon>
                    </template>
                    <template #item="{ item, props }">
                      <v-list-item v-bind="props">
                        <template #subtitle>
                          <span class="text-body-small">
                            {{ item.game_versions?.slice(0, 3).join(', ') }}
                            {{ item.game_versions?.length > 3 ? '...' : '' }}
                          </span>
                        </template>
                      </v-list-item>
                    </template>
                  </v-select>
                </v-col>
              </v-row>
            </div>

            <div class="config-section mb-4">
              <div class="text-title-small text-on-surface-variant mb-3">
                <v-icon size="18" class="mr-1">mdi-folder-cog</v-icon>
                {{ t('installModpack.instanceConfig') }}
              </div>

              <v-text-field
                v-model="instanceName"
                :label="t('installModpack.instanceNameLabel')"
                :placeholder="title"
                density="comfortable"
                variant="outlined"
                :error-messages="instanceNameError || undefined"
                :loading="checkingInstanceName"
                hide-details="auto"
                counter="64"
                maxlength="64"
              >
                <template #prepend-inner>
                  <v-icon size="18" color="on-surface-variant">mdi-rename</v-icon>
                </template>
                <template #append-inner>
                  <v-fade-transition>
                    <v-icon
                      v-if="instanceName && !instanceNameError && !checkingInstanceName"
                      size="18"
                      color="success"
                    >
                      mdi-check-circle
                    </v-icon>
                  </v-fade-transition>
                </template>
              </v-text-field>

              <div class="text-body-small text-on-surface-variant mt-2">{{ t('installModpack.instanceNameHint') }}</div>
            </div>

            <v-expand-transition>
              <v-card v-if="selectedVersionInfo" color="surface-container-high" variant="flat" class="mb-4">
                <v-card-text class="pa-3">
                  <div class="d-flex align-center justify-space-between">
                    <div>
                      <div class="text-body-medium font-weight-medium">{{ selectedVersionInfo.name }}</div>
                      <div class="text-body-small text-on-surface-variant">
                        {{ t('installModpack.versionNumber', { version: selectedVersionInfo.version_number }) }}
                      </div>
                    </div>
                    <div class="d-flex ga-1">
                      <v-chip
                        v-for="loader in selectedVersionInfo.loaders.slice(0, 2)"
                        :key="loader"
                        size="x-small"
                        color="primary"
                        variant="tonal"
                      >
                        {{ loader }}
                      </v-chip>
                    </div>
                  </div>
                </v-card-text>
              </v-card>
            </v-expand-transition>
          </template>
        </template>

        <div class="d-flex justify-end ga-2 mt-4">
          <v-btn v-if="!installing" variant="text" @click="goBack">{{ t('installModpack.cancel') }}</v-btn>
          <v-btn
            v-if="!installing"
            color="primary"
            :disabled="!canInstall"
            :loading="checkingInstanceName"
            @click="install"
          >
            <v-icon start>mdi-download</v-icon>
            {{ t('installModpack.install') }}
          </v-btn>
        </div>
      </v-card-text>
    </v-card>
  </v-container>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from 'vue';
import { useRoute, useRouter, onBeforeRouteLeave } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { useMultiEventSubscription } from '../composables/useEventSubscription';
import { useNotificationStore } from '../stores/notificationStore';
import { useSettingsStore } from '../stores/settings';
import { modpackApi, instanceApi, versionApi, curseforgeApi } from '../services';
import type { CfManualFile } from '../types/events';
import { copyText } from '../utils/clipboard';
import { logError } from '../utils/logger';
import {
  compareVersionDesc,
  formatBytesPerSecond,
  formatBytesProgress,
  getErrorMessage,
  getErrorCode,
} from '../utils/format';
import type {
  DownloadProgress,
  InstallProgressPayload,
  LocalModpackPreview,
  ModpackActiveFile,
  ModpackFilesProgress,
  ModrinthVersion,
} from '../types/events';

const route = useRoute();
const router = useRouter();
const notificationStore = useNotificationStore();
const settingsStore = useSettingsStore();
const { t } = useI18n();

/** 本地导入时 query 里带 file，在线安装时带 projectId + source */
const localFilePath = String(route.query.file || '');
const isLocal = computed(() => localFilePath.length > 0);
const projectId = String(route.query.projectId || '');
const title = String(route.query.title || '');
const isCurseforge = String(route.query.source || 'modrinth') === 'curseforge';
const sourceLabel = isCurseforge ? 'CurseForge' : 'Modrinth';

const loadingVersions = ref(false);
const modpackVersions = ref<ModrinthVersion[]>([]);
const localPreview = ref<LocalModpackPreview | null>(null);
const localError = ref('');
const selectedVersionId = ref<string | null>(null);
const selectedLoader = ref<string | null>(null);
const selectedGameVersion = ref<string | null>(null);
const instanceName = ref<string>('');
const installing = ref(false);
const cancelling = ref(false);
const installProgress = ref<InstallProgressPayload>({
  progress: 0,
  message: '',
  indeterminate: false,
});
const fileProgress = ref<ModpackFilesProgress | null>(null);
const versionDownload = ref<DownloadProgress | null>(null);
const versionDownloadDone = ref(false);
const validatingFiles = ref(false);
const postInstallChecked = ref(false);
const missingFileCount = ref(0);
const completingFiles = ref(false);
const completionError = ref<string | null>(null);
const awaitingPostInstall = ref(false);
/** CurseForge 受限文件清单：安装本体已完成，但这些文件要用户手动下载后放进 mods */
const manualDownloads = ref<CfManualFile[]>([]);

const instanceNameError = ref<string | null>(null);
const checkingInstanceName = ref(false);

let instanceNameCheckTimeout: ReturnType<typeof setTimeout> | null = null;

const activeFiles = computed(() => fileProgress.value?.files ?? []);
// 模组下载与游戏核心并行：模组进度走 55..74，加载器不确定态在 74，核心下载的
// download-progress 事件另行显示。两段并行期间同时展示文件列表与核心进度，
// 都完成后再由后端推到 90
const filesStage = computed(
  () => installing.value && installProgress.value.progress >= 55 && installProgress.value.progress < 90,
);
const gameStage = computed(
  () => installing.value && installProgress.value.progress >= 55 && installProgress.value.progress < 90,
);
const queuedFileCount = computed(() => {
  const progress = fileProgress.value;
  if (!progress) return 0;
  return Math.max(0, progress.total_files - progress.completed_files - progress.files.length);
});
const etaText = computed(() => {
  const progress = fileProgress.value;
  if (!progress || progress.speed <= 0 || progress.total_bytes <= progress.downloaded_bytes) return '';

  const seconds = Math.ceil((progress.total_bytes - progress.downloaded_bytes) / progress.speed);
  if (!Number.isFinite(seconds) || seconds <= 0) return '';
  if (seconds >= 3600) {
    return t('installModpack.remainingHm', {
      hours: Math.floor(seconds / 3600),
      minutes: Math.floor((seconds % 3600) / 60),
    });
  }
  if (seconds >= 60) {
    return t('installModpack.remainingMs', { minutes: Math.floor(seconds / 60), seconds: seconds % 60 });
  }
  return t('installModpack.remainingS', { seconds });
});

const installSubscriptions = useMultiEventSubscription();
installSubscriptions.add<InstallProgressPayload>('modpack-install-progress', (event) => {
  installProgress.value = event.payload;
});
installSubscriptions.add<ModpackFilesProgress>('modpack-file-progress', (event) => {
  fileProgress.value = event.payload;
});
installSubscriptions.add<DownloadProgress>('download-progress', (event) => {
  versionDownload.value = event.payload;
  if (event.payload.status === 'completed') {
    versionDownloadDone.value = true;
  }
});

const selectedVersionInfo = computed(() => {
  if (!selectedVersionId.value) return null;
  return modpackVersions.value.find((v) => v.id === selectedVersionId.value) || null;
});

const effectiveInstanceName = computed(() => {
  return instanceName.value.trim() || localPreview.value?.suggestedInstanceName || title;
});

/** 没填实例名时用来校验/预览的默认名：本地包优先用清单里名字的净化结果 */
const fallbackInstanceName = computed(() => localPreview.value?.suggestedInstanceName || title);

const localLoaderLabel = computed(() => {
  const loader = localPreview.value?.loader;
  if (!loader) return t('localModpack.vanilla');
  const version = localPreview.value?.loaderVersion;
  return version ? `${loader} ${version}` : loader;
});

watch(instanceName, (newName) => {
  if (instanceNameCheckTimeout) {
    clearTimeout(instanceNameCheckTimeout);
  }

  instanceNameError.value = null;

  const nameToCheck = newName.trim() || fallbackInstanceName.value;
  if (!nameToCheck) return;

  instanceNameCheckTimeout = setTimeout(async () => {
    await checkInstanceName(nameToCheck);
  }, 300);
});

// 每次发起校验递增：慢的旧校验后到时不得覆盖新结论、也不得提前解除安装按钮的禁用
let instanceNameCheckSeq = 0;

async function checkInstanceName(name: string) {
  if (!name) return;

  checkingInstanceName.value = true;
  const seq = ++instanceNameCheckSeq;
  try {
    const result = await instanceApi.checkInstanceNameAvailable(name);
    if (seq !== instanceNameCheckSeq) return;
    instanceNameError.value = result.is_valid ? null : result.error_message;
  } catch (e) {
    logError('Failed to check instance name', e, 'InstallModpackView');
  } finally {
    if (seq === instanceNameCheckSeq) {
      checkingInstanceName.value = false;
    }
  }
}

const canInstall = computed(() => {
  if (isLocal.value) {
    return Boolean(localPreview.value) && !instanceNameError.value && !checkingInstanceName.value;
  }
  return (
    selectedVersionId.value &&
    !instanceNameError.value &&
    !checkingInstanceName.value &&
    modpackVersions.value.length > 0
  );
});

onBeforeRouteLeave(() => {
  if (installing.value && !cancelling.value) {
    notificationStore.warning(t('installModpack.installInProgress'), t('installModpack.installInProgressHint'));
    return false;
  }
  return true;
});

function goBack() {
  if (installing.value && !cancelling.value) {
    notificationStore.warning(t('installModpack.installInProgress'), t('installModpack.installInProgressHint'));
    return;
  }
  router.back();
}

async function cancelInstall() {
  if (cancelling.value) return;
  cancelling.value = true;
  try {
    await modpackApi.cancelModpackInstall();
    notificationStore.info(t('installModpack.cancelling'), t('installModpack.cancellingHint'));
  } catch (e) {
    logError('Failed to cancel install', e, 'InstallModpackView');
  }
}

const loaderOptions = computed(() => {
  const loaders = new Set<string>();
  for (const v of modpackVersions.value) {
    for (const loader of v.loaders) {
      loaders.add(loader);
    }
  }
  const priority = ['forge', 'neoforge', 'fabric', 'quilt'];
  return Array.from(loaders).sort((a, b) => {
    const ai = priority.indexOf(a.toLowerCase());
    const bi = priority.indexOf(b.toLowerCase());
    return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
  });
});

const filteredGameVersions = computed(() => {
  if (!selectedLoader.value) return [];

  const versions = new Set<string>();
  for (const v of modpackVersions.value) {
    if (v.loaders.some((l) => l.toLowerCase() === selectedLoader.value?.toLowerCase())) {
      for (const gv of v.game_versions) {
        versions.add(gv);
      }
    }
  }
  return Array.from(versions).sort(compareVersionDesc);
});

const filteredModpackVersions = computed(() => {
  if (!selectedLoader.value) return [];

  let filtered = modpackVersions.value.filter((v) =>
    v.loaders.some((l) => l.toLowerCase() === selectedLoader.value?.toLowerCase()),
  );

  if (selectedGameVersion.value) {
    filtered = filtered.filter((v) => v.game_versions.includes(selectedGameVersion.value as string));
  }

  return filtered.map((v) => ({
    ...v,
    // CurseForge 的文件即版本模型里 name 与 version_number 同源，拼在一起会重复
    displayName:
      v.name === v.version_number ? v.name : `${v.name} (${v.version_number})`,
  }));
});

function onLoaderChange() {
  selectedGameVersion.value = filteredGameVersions.value[0] || null;
  onGameVersionChange();
}

function onGameVersionChange() {
  const first = filteredModpackVersions.value[0];
  selectedVersionId.value = first ? first.id : null;
}

async function loadVersions() {
  if (!projectId) return;
  loadingVersions.value = true;
  try {
    const versions = isCurseforge
      ? await curseforgeApi.getModpackVersions(projectId)
      : await modpackApi.getModrinthModpackVersions(projectId);
    modpackVersions.value = versions || [];

    if (loaderOptions.value.length > 0) {
      selectedLoader.value = loaderOptions.value[0];
      onLoaderChange();
    }
  } catch (e) {
    logError('Failed to load modpack versions', e, 'InstallModpackView');
    notificationStore.error(t('installModpack.loadFailed'), t('installModpack.loadFailedHint'));
  } finally {
    loadingVersions.value = false;
  }
}

async function loadLocalPreview() {
  loadingVersions.value = true;
  localError.value = '';
  try {
    localPreview.value = await modpackApi.previewLocalModpack(localFilePath);
  } catch (e) {
    logError('Failed to read local modpack', e, 'InstallModpackView');
    localError.value = getErrorMessage(e);
  } finally {
    loadingVersions.value = false;
  }
}

async function install() {
  if (!canInstall.value) return;
  if (isLocal.value ? !localFilePath : !projectId || !selectedVersionId.value) return;

  installing.value = true;
  installProgress.value = { progress: 0, message: t('installModpack.prepareInstall'), indeterminate: false };
  fileProgress.value = null;
  versionDownload.value = null;
  versionDownloadDone.value = false;
  validatingFiles.value = false;
  postInstallChecked.value = false;
  missingFileCount.value = 0;
  completingFiles.value = false;
  completionError.value = null;
  awaitingPostInstall.value = false;
  manualDownloads.value = [];

  try {
    // 订阅失败也要走 catch/finally，别让 installing 一直停在任务中
    await installSubscriptions.subscribeAll();
    if (isLocal.value) {
      const warnings = await modpackApi.installLocalModpack({
        filePath: localFilePath,
        instanceName: effectiveInstanceName.value,
      });
      manualDownloads.value = warnings || [];
    } else {
      const installOptions = {
        modpack_id: projectId,
        version_id: selectedVersionId.value!,
        instance_name: effectiveInstanceName.value,
        install_path: '',
      };
      // CF 安装返回受限文件清单；Modrinth 安装没有返回值
      manualDownloads.value = isCurseforge
        ? await curseforgeApi.installModpack(installOptions)
        : [];
      if (!isCurseforge) {
        await modpackApi.installModrinthModpack(installOptions);
      }
    }

    installProgress.value = { progress: 100, message: t('installModpack.installDone'), indeterminate: false };

    // 受限文件先亮出来让用户处理，确认后再走校验/补齐
    if (manualDownloads.value.length > 0) {
      return;
    }
    await runPostInstall();
  } catch (e) {
    if (getErrorCode(e) === 'cancelled') {
      notificationStore.info(t('installModpack.cancelledTitle'), t('installModpack.installCancelled'));
    } else {
      logError('Failed to install modpack', e, 'InstallModpackView');
      notificationStore.error(t('installModpack.installFailed'), getErrorMessage(e), true);
    }
  } finally {
    cancelling.value = false;
    if (!awaitingPostInstall.value && manualDownloads.value.length === 0) {
      installSubscriptions.unsubscribeAll();
      installing.value = false;
    }
  }
}

/** 安装完成后的校验与原版文件补齐；受限文件确认后也回到这里 */
async function runPostInstall() {
  const acknowledgedManual = manualDownloads.value.length > 0;
  validatingFiles.value = true;
  let validationFailed = false;
  try {
    const missing = await versionApi.validateVersionFiles(effectiveInstanceName.value);
    missingFileCount.value = missing.length;
  } catch (e) {
    validationFailed = true;
    logError('Failed to validate installed files', e, 'InstallModpackView');
  } finally {
    validatingFiles.value = false;
    postInstallChecked.value = true;
  }

  if (missingFileCount.value === 0) {
    finishInstall(
      acknowledgedManual
        ? t('installModpack.installDoneManualFiles', {
            name: effectiveInstanceName.value,
            count: manualDownloads.value.length,
          })
        : validationFailed
          ? t('installModpack.installDoneUnvalidated', { name: effectiveInstanceName.value })
          : t('installModpack.installDoneValidated', { name: effectiveInstanceName.value }),
    );
    return;
  }

  // 默认直接补齐缺失的原版文件，不再让用户确认
  awaitingPostInstall.value = true;
  void completeMissingFiles();
}

async function copyManualUrls() {
  const urls = manualDownloads.value.map((file) => file.url).filter(Boolean).join('\n');
  const ok = await copyText(urls);
  if (ok) {
    notificationStore.info(t('installModpack.urlsCopied'), undefined);
  }
}

async function completeMissingFiles() {
  if (completingFiles.value) return;

  completingFiles.value = true;
  completionError.value = null;
  versionDownload.value = null;
  installProgress.value = { progress: 100, message: t('installModpack.completingFiles'), indeterminate: true };

  try {
    const mirror = settingsStore.downloadMirror || undefined;
    await versionApi.downloadVersion(effectiveInstanceName.value, mirror);
    finishInstall(t('installModpack.installDoneCompleted', { name: effectiveInstanceName.value }));
  } catch (e) {
    installProgress.value = { progress: 100, message: t('installModpack.filesNotCompleted'), indeterminate: false };
    if (getErrorCode(e) === 'cancelled') {
      completionError.value = t('installModpack.completionStopped');
    } else {
      logError('Failed to complete game files', e, 'InstallModpackView');
      completionError.value = getErrorMessage(e);
      notificationStore.error(t('installModpack.completionFailed'), getErrorMessage(e), true);
    }
  } finally {
    completingFiles.value = false;
    cancelling.value = false;
  }
}

async function stopCompletion() {
  if (cancelling.value) return;
  cancelling.value = true;
  try {
    await versionApi.cancelDownload();
    notificationStore.info(t('installModpack.stopping'), t('installModpack.stoppingHint'));
  } catch (e) {
    logError('Failed to stop completion', e, 'InstallModpackView');
    cancelling.value = false;
  }
}

function finishInstall(message: string) {
  notificationStore.success(t('installModpack.installSuccess'), message);
  closeInstallPanel();
  router.push('/instance-manager');
}

function finishLater() {
  notificationStore.warning(
    t('installModpack.installFinished'),
    t('installModpack.finishLaterHint', { name: effectiveInstanceName.value }),
  );
  closeInstallPanel();
  router.push('/instance-manager');
}

function closeInstallPanel() {
  awaitingPostInstall.value = false;
  installing.value = false;
  cancelling.value = false;
  installSubscriptions.unsubscribeAll();
  validatingFiles.value = false;
  postInstallChecked.value = false;
  missingFileCount.value = 0;
  completingFiles.value = false;
  completionError.value = null;
  versionDownload.value = null;
  manualDownloads.value = [];
}

function formatBytes(bytes: number): string {
  return formatBytesProgress(bytes);
}

function formatSpeed(bytesPerSecond: number): string {
  return formatBytesPerSecond(bytesPerSecond);
}

function filePercent(file: ModpackActiveFile): number {
  if (file.total <= 0) return 0;
  return Math.min(100, (file.downloaded / file.total) * 100);
}

function fileSizeLabel(file: ModpackActiveFile): string {
  return file.total > 0 ? `${formatBytes(file.downloaded)} / ${formatBytes(file.total)}` : formatBytes(file.downloaded);
}

function baseName(path: string): string {
  const index = path.lastIndexOf('/');
  return index === -1 ? path : path.slice(index + 1);
}

function dirName(path: string): string {
  const index = path.lastIndexOf('/');
  return index === -1 ? '' : path.slice(0, index);
}

onMounted(async () => {
  if (isLocal.value) {
    await loadLocalPreview();
  } else {
    await loadVersions();
  }

  const nameToCheck = title || fallbackInstanceName.value;
  if (nameToCheck) {
    await checkInstanceName(nameToCheck);
  }
});

onUnmounted(() => {
  if (instanceNameCheckTimeout) {
    clearTimeout(instanceNameCheckTimeout);
  }
});
</script>

<style scoped>
.config-section {
  padding: 16px;
  background: rgb(var(--v-theme-surface-container-low));
  border-radius: 12px;
}

.file-list {
  max-height: 300px;
  overflow-y: auto;
  padding-right: 4px;
}

.manual-file + .manual-file {
  margin-top: 2px;
}

.file-row {
  padding: 8px 10px;
  margin-bottom: 6px;
  border-radius: 8px;
  background: rgb(var(--v-theme-surface-container));
}

.file-row:last-child {
  margin-bottom: 0;
}

.file-name {
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-dir {
  margin-top: 1px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-row-enter-active,
.file-row-leave-active {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}

.file-row-enter-from,
.file-row-leave-to {
  opacity: 0;
  transform: translateX(8px);
}
</style>
