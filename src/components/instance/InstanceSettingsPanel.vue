<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { useI18n } from 'vue-i18n';
import { useInstanceScope } from '../../composables/useInstanceScope';
import { useDebounceFn, watchDebounced } from '../../composables/useDebounce';
import { useNotificationStore } from '../../stores/notificationStore';
import { useSettingsStore } from '../../stores/settings';
import { configApi, instanceApi, javaApi } from '../../services';
import type { GameConfig, InstanceSettings } from '../../types/events';
import { getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const props = defineProps<{ instanceName: string }>();
const { t } = useI18n();
const notificationStore = useNotificationStore();
const settingsStore = useSettingsStore();
const scope = useInstanceScope(() => props.instanceName);

const loading = ref(true);
const saving = ref(false);
const settings = ref<InstanceSettings>(emptySettings());
const globalConfig = ref<GameConfig | null>(null);
const javaPaths = ref<string[]>([]);
const javaVersions = ref<Record<string, string>>({});
const memoryWarning = ref('');

const windowPresets = [
  { width: 854, height: 480, label: '854 × 480' },
  { width: 1280, height: 720, label: '1280 × 720' },
  { width: 1600, height: 900, label: '1600 × 900' },
  { width: 1920, height: 1080, label: '1920 × 1080' },
];

function emptySettings(): InstanceSettings {
  return { javaPath: null, memoryMb: null, jvmExtraArgs: null, windowWidth: null, windowHeight: null };
}

const followJava = computed({
  get: () => settings.value.javaPath === null,
  set: (value: boolean) => {
    settings.value.javaPath = value ? null : (globalConfig.value?.java_path ?? javaPaths.value[0] ?? '');
    scheduleSave();
  },
});

const followMemory = computed({
  get: () => settings.value.memoryMb === null,
  set: (value: boolean) => {
    settings.value.memoryMb = value ? null : (globalConfig.value?.max_memory ?? 4096);
    scheduleSave();
  },
});

const followJvm = computed({
  get: () => settings.value.jvmExtraArgs === null,
  set: (value: boolean) => {
    settings.value.jvmExtraArgs = value ? null : (globalConfig.value?.jvm_advanced?.extraArgs ?? '');
    scheduleSave();
  },
});

const followWindow = computed({
  get: () => settings.value.windowWidth === null && settings.value.windowHeight === null,
  set: (value: boolean) => {
    if (value) {
      settings.value.windowWidth = null;
      settings.value.windowHeight = null;
    } else {
      settings.value.windowWidth = globalConfig.value?.window_width ?? 1280;
      settings.value.windowHeight = globalConfig.value?.window_height ?? 720;
    }
    scheduleSave();
  },
});

const maxAllocatableMB = computed(() => Math.max(512, settingsStore.totalMemoryMB || 16384));

const globalMemoryLabel = computed(() => {
  const config = globalConfig.value;
  if (!config) return '—';
  if (config.auto_memory_enabled) return t('instanceDetail.instanceSettings.autoMemory');
  return `${config.max_memory ?? 4096} MB`;
});

const globalWindowLabel = computed(() => {
  const config = globalConfig.value;
  if (!config) return '—';
  if (!config.window_width || !config.window_height) {
    return t('instanceDetail.instanceSettings.windowDefault');
  }
  return `${config.window_width} × ${config.window_height}`;
});

const globalJavaLabel = computed(() => {
  const config = globalConfig.value;
  if (!config) return '—';
  if (config.auto_match_java) return t('instanceDetail.instanceSettings.javaAutoMatch');
  return config.java_path || '—';
});

const globalJvmLabel = computed(() => {
  const extra = globalConfig.value?.jvm_advanced?.extraArgs?.trim();
  return extra || t('instanceDetail.instanceSettings.jvmEmpty');
});

const save = useDebounceFn(() => void persist(), 500);
let pendingInstanceName = props.instanceName;

function scheduleSave() {
  pendingInstanceName = props.instanceName;
  if (!loading.value) save.call();
}

async function persist() {
  const instanceName = pendingInstanceName;
  const snapshot = { ...settings.value };
  saving.value = true;
  try {
    await instanceApi.setInstanceSettings(instanceName, snapshot);
  } catch (e) {
    logError('Failed to save instance settings', e, 'InstanceSettingsPanel');
    if (instanceName === props.instanceName) {
      notificationStore.error(t('instanceDetail.instanceSettings.saveFailed'), getErrorMessage(e));
    }
  } finally {
    saving.value = false;
  }
}

async function load() {
  const task = scope.begin();
  loading.value = true;
  try {
    const [stored, config, discovered] = await Promise.all([
      instanceApi.getInstanceSettings(task.instanceName),
      configApi.getConfig(),
      javaApi.findJavaInstallations().catch(() => []),
      settingsStore.loadSystemMemory(),
    ]);
    if (!scope.isCurrent(task)) return;

    settings.value = stored;
    globalConfig.value = config;

    const custom = await javaApi.getCustomJavaPaths().catch(() => []);
    if (!scope.isCurrent(task)) return;
    const paths = [...new Set([...discovered, ...custom])];
    if (stored.javaPath && !paths.includes(stored.javaPath)) {
      paths.unshift(stored.javaPath);
    }
    javaPaths.value = paths;

    const versions = await Promise.all(
      paths.map(async (path) => [path, await javaApi.getJavaVersion(path).catch(() => '')] as const),
    );
    if (!scope.isCurrent(task)) return;
    javaVersions.value = Object.fromEntries(versions);
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load instance settings', e, 'InstanceSettingsPanel');
    notificationStore.error(t('instanceDetail.instanceSettings.loadFailed'), getErrorMessage(e));
  } finally {
    if (scope.isCurrent(task)) loading.value = false;
  }
}

async function refreshMemoryWarning() {
  if (loading.value || settings.value.memoryMb === null) {
    memoryWarning.value = '';
    return;
  }
  try {
    memoryWarning.value = (await configApi.checkMemoryWarning(settings.value.memoryMb)) || '';
  } catch (e) {
    logError('Failed to check instance memory warning', e, 'InstanceSettingsPanel');
    memoryWarning.value = '';
  }
}

watchDebounced(
  () => settings.value.memoryMb,
  () => void refreshMemoryWarning(),
);

async function browseJava() {
  const selected = await open({
    multiple: false,
    title: t('instanceDetail.instanceSettings.javaBrowse'),
    filters: [{ name: 'Java', extensions: ['exe', ''] }],
  });
  if (!selected || Array.isArray(selected)) return;
  const valid = await javaApi.validateJavaPath(selected);
  if (!valid) {
    notificationStore.warning(t('instanceDetail.instanceSettings.javaInvalid'));
    return;
  }
  if (!javaPaths.value.includes(selected)) {
    javaPaths.value = [selected, ...javaPaths.value];
    javaVersions.value[selected] = await javaApi.getJavaVersion(selected).catch(() => '');
  }
  settings.value.javaPath = selected;
  pendingInstanceName = props.instanceName;
  await persist();
}

function selectJava(path: string) {
  settings.value.javaPath = path;
  scheduleSave();
}

function toggleFollowJava(value: boolean | null) {
  followJava.value = value === true;
}

function toggleFollowMemory(value: boolean | null) {
  followMemory.value = value === true;
}

function toggleFollowJvm(value: boolean | null) {
  followJvm.value = value === true;
}

function toggleFollowWindow(value: boolean | null) {
  followWindow.value = value === true;
}

function setMemoryValue(value: number | number[]) {
  const raw = Array.isArray(value) ? value[0] : value;
  const clamped = Math.min(Math.max(Math.round(raw) || 512, 512), maxAllocatableMB.value);
  settings.value.memoryMb = clamped;
  scheduleSave();
}

function onMemoryInput(event: Event) {
  setMemoryValue(Number((event.target as HTMLInputElement).value));
}

function setWindowValue(width: number, height: number) {
  settings.value.windowWidth = Math.min(Math.max(Math.round(width) || 854, 640), 7680);
  settings.value.windowHeight = Math.min(Math.max(Math.round(height) || 480, 480), 4320);
  scheduleSave();
}

function onWindowWidthInput(event: Event) {
  setWindowValue(Number((event.target as HTMLInputElement).value), settings.value.windowHeight ?? 720);
}

function onWindowHeightInput(event: Event) {
  setWindowValue(settings.value.windowWidth ?? 1280, Number((event.target as HTMLInputElement).value));
}

function onJvmInput(value: string) {
  settings.value.jvmExtraArgs = value;
  scheduleSave();
}

onMounted(load);
watch(
  () => props.instanceName,
  () => {
    // 切换实例前先把旧实例的待保存改动落盘，避免改到新实例头上
    save.flush();
    void load();
  },
);
</script>

<template>
  <div class="settings-group mb-4">
    <div class="d-flex align-center justify-space-between mb-2">
      <div class="text-title-medium">{{ t('instanceDetail.instanceSettings.settingsTitle') }}</div>
      <v-progress-circular v-if="saving" indeterminate size="18" width="2" color="primary" />
    </div>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-2">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-file-cog-outline</v-icon>
            <div>
              <div class="text-title-small">{{ t('instanceDetail.instanceSettings.javaTitle') }}</div>
              <div class="text-body-small text-on-surface-variant">
                {{ t('instanceDetail.instanceSettings.javaHint') }}
              </div>
            </div>
          </div>
          <v-switch
            :model-value="followJava"
            @update:model-value="toggleFollowJava"
            :aria-label="t('instanceDetail.instanceSettings.followGlobal')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <div v-if="followJava" class="text-body-medium text-on-surface-variant">
          {{ t('instanceDetail.instanceSettings.followGlobal') }}：{{ globalJavaLabel }}
        </div>
        <div v-else>
          <div class="text-body-small text-on-surface-variant mb-2">
            {{ t('instanceDetail.instanceSettings.followGlobal') }}：{{ globalJavaLabel }}
          </div>
          <v-list
            v-if="javaPaths.length > 0"
            density="compact"
            lines="two"
            class="java-list"
            bg-color="surface-container-high"
          >
            <v-list-item
              v-for="path in javaPaths"
              :key="path"
              :active="settings.javaPath === path"
              @click="selectJava(path)"
            >
              <template #prepend>
                <v-icon :color="settings.javaPath === path ? 'primary' : 'on-surface-variant'">
                  {{ settings.javaPath === path ? 'mdi-radiobox-marked' : 'mdi-radiobox-blank' }}
                </v-icon>
              </template>
              <v-list-item-title class="text-body-medium font-mono">{{ path }}</v-list-item-title>
              <v-list-item-subtitle v-if="javaVersions[path]" class="text-body-small">
                {{ javaVersions[path] }}
              </v-list-item-subtitle>
            </v-list-item>
          </v-list>
          <v-alert v-else color="warning-container" density="compact" class="mb-2">
            <span class="text-on-warning-container">{{ t('instanceDetail.instanceSettings.javaNone') }}</span>
          </v-alert>
          <v-btn
            variant="tonal"
            color="primary"
            size="small"
            prepend-icon="mdi-folder-open-outline"
            @click="browseJava"
          >
            {{ t('instanceDetail.instanceSettings.javaBrowse') }}
          </v-btn>
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-2">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-memory</v-icon>
            <div>
              <div class="text-title-small">{{ t('instanceDetail.instanceSettings.memoryTitle') }}</div>
              <div class="text-body-small text-on-surface-variant">
                {{ t('instanceDetail.instanceSettings.memoryHint') }}
              </div>
            </div>
          </div>
          <v-switch
            :model-value="followMemory"
            @update:model-value="toggleFollowMemory"
            :aria-label="t('instanceDetail.instanceSettings.followGlobal')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <div v-if="followMemory" class="text-body-medium text-on-surface-variant">
          {{ t('instanceDetail.instanceSettings.followGlobal') }}：{{ globalMemoryLabel }}
        </div>
        <div v-else>
          <div class="d-flex align-center justify-space-between mb-2">
            <span class="text-body-small text-on-surface-variant">{{
              t('instanceDetail.instanceSettings.memoryPresetLabel')
            }}</span>
            <span class="text-body-medium num-mono">{{ settings.memoryMb ?? 4096 }} MB</span>
          </div>
          <div class="d-flex flex-wrap ga-2 mb-3">
            <v-chip
              v-for="preset in [2048, 4096, 6144, 8192]"
              :key="preset"
              :color="settings.memoryMb === preset ? 'primary' : 'secondary'"
              :variant="settings.memoryMb === preset ? 'flat' : 'tonal'"
              :disabled="preset > maxAllocatableMB"
              @click="setMemoryValue(preset)"
            >
              {{ preset / 1024 }} GB
            </v-chip>
          </div>
          <v-slider
            :model-value="settings.memoryMb ?? 4096"
            :min="512"
            :max="maxAllocatableMB"
            :step="128"
            hide-details
            color="primary"
            @update:model-value="setMemoryValue"
          />
          <v-text-field
            :model-value="settings.memoryMb ?? 4096"
            type="number"
            density="compact"
            :aria-label="t('instanceDetail.instanceSettings.memoryTitle')"
            suffix="MB"
            hide-details
            hide-spin-buttons
            class="mt-3"
            style="max-width: 180px"
            @change="onMemoryInput"
          />
          <v-alert v-if="memoryWarning" color="warning-container" density="compact" class="mt-3">
            <span class="text-on-warning-container">{{ memoryWarning }}</span>
          </v-alert>
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-2">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-console-line</v-icon>
            <div>
              <div class="text-title-small">{{ t('instanceDetail.instanceSettings.jvmTitle') }}</div>
              <div class="text-body-small text-on-surface-variant">
                {{ t('instanceDetail.instanceSettings.jvmHint') }}
              </div>
            </div>
          </div>
          <v-switch
            :model-value="followJvm"
            @update:model-value="toggleFollowJvm"
            :aria-label="t('instanceDetail.instanceSettings.followGlobal')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <div v-if="followJvm" class="text-body-medium text-on-surface-variant">
          {{ t('instanceDetail.instanceSettings.followGlobal') }}：{{ globalJvmLabel }}
        </div>
        <v-textarea
          v-else
          :model-value="settings.jvmExtraArgs ?? ''"
          :label="t('instanceDetail.instanceSettings.jvmTitle')"
          :placeholder="t('instanceDetail.instanceSettings.jvmPlaceholder')"
          rows="3"
          density="compact"
          variant="outlined"
          hide-details
          class="font-mono-input"
          @update:model-value="onJvmInput"
        />
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-2">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-resize</v-icon>
            <div>
              <div class="text-title-small">{{ t('instanceDetail.instanceSettings.windowTitle') }}</div>
              <div class="text-body-small text-on-surface-variant">
                {{ t('instanceDetail.instanceSettings.windowHint') }}
              </div>
            </div>
          </div>
          <v-switch
            :model-value="followWindow"
            @update:model-value="toggleFollowWindow"
            :aria-label="t('instanceDetail.instanceSettings.followGlobal')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <div v-if="followWindow" class="text-body-medium text-on-surface-variant">
          {{ t('instanceDetail.instanceSettings.followGlobal') }}：{{ globalWindowLabel }}
        </div>
        <div v-else>
          <div class="text-body-small text-on-surface-variant mb-2">
            {{ t('instanceDetail.instanceSettings.windowPresetLabel') }}
          </div>
          <div class="d-flex flex-wrap ga-2 mb-3">
            <v-chip
              v-for="preset in windowPresets"
              :key="preset.label"
              :color="
                settings.windowWidth === preset.width && settings.windowHeight === preset.height
                  ? 'primary'
                  : 'secondary'
              "
              :variant="
                settings.windowWidth === preset.width && settings.windowHeight === preset.height ? 'flat' : 'tonal'
              "
              @click="setWindowValue(preset.width, preset.height)"
            >
              {{ preset.label }}
            </v-chip>
          </div>
          <v-row density="compact">
            <v-col cols="6">
              <v-text-field
                :model-value="settings.windowWidth ?? 1280"
                type="number"
                density="compact"
                :label="t('instanceDetail.instanceSettings.windowWidth')"
                suffix="px"
                hide-details
                hide-spin-buttons
                @change="onWindowWidthInput"
              />
            </v-col>
            <v-col cols="6">
              <v-text-field
                :model-value="settings.windowHeight ?? 720"
                type="number"
                density="compact"
                :label="t('instanceDetail.instanceSettings.windowHeight')"
                suffix="px"
                hide-details
                hide-spin-buttons
                @change="onWindowHeightInput"
              />
            </v-col>
          </v-row>
        </div>
      </v-card-text>
    </v-card>

    <div class="text-body-small text-on-surface-variant">
      {{ t('instanceDetail.instanceSettings.effectiveNextLaunch') }}
    </div>
  </div>
</template>

<style scoped>
.java-list {
  max-height: 220px;
  overflow-y: auto;
}

.font-mono {
  font-family: 'Consolas', 'Monaco', monospace;
  font-size: 0.875rem;
}

.font-mono-input :deep(input),
.font-mono-input :deep(textarea) {
  font-family: 'Consolas', 'Monaco', monospace;
  font-size: 0.875rem;
}
</style>
