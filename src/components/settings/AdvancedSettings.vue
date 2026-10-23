<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { configApi } from '../../services';
import { useSettingsStore } from '../../stores/settings';
import { useDebounceFn } from '../../composables/useDebounce';
import { logError } from '../../utils/logger';
import type { JvmAdvancedOptions, GcStrategy } from '../../types/events';

const settingsStore = useSettingsStore();
const { t } = useI18n();

const options = ref<JvmAdvancedOptions>({
  gcStrategy: 'g1',
  alwaysPreTouch: false,
  disableExplicitGc: false,
  stringDeduplication: false,
  perfDisableSharedMem: false,
  metaspaceSizeMb: null,
  metaspaceMaxSizeMb: null,
  heapDumpOnOom: false,
  extraArgs: '',
});

const metaspaceEnabled = ref(false);
const isInitialized = ref(false);
const saving = ref(false);
const showPreview = ref(false);
const effectiveArgs = ref<string[]>([]);

const metaspacePresets = [128, 256, 512];
const metaspaceMaxPresets = [256, 512, 768, 1024];

// 上限低于 256MB 时大型整合包很容易直接 Metaspace OOM
const metaspaceTooSmall = computed(
  () => options.value.metaspaceMaxSizeMb !== null && options.value.metaspaceMaxSizeMb < 256,
);

const gcOptions = computed<{ value: GcStrategy; title: string; subtitle: string }[]>(() => [
  {
    value: 'g1',
    title: t('settings.advanced.gcOptions.g1Title'),
    subtitle: t('settings.advanced.gcOptions.g1Subtitle'),
  },
  {
    value: 'default',
    title: t('settings.advanced.gcOptions.defaultTitle'),
    subtitle: t('settings.advanced.gcOptions.defaultSubtitle'),
  },
  {
    value: 'parallel',
    title: t('settings.advanced.gcOptions.parallelTitle'),
    subtitle: t('settings.advanced.gcOptions.parallelSubtitle'),
  },
  {
    value: 'serial',
    title: t('settings.advanced.gcOptions.serialTitle'),
    subtitle: t('settings.advanced.gcOptions.serialSubtitle'),
  },
  { value: 'z', title: t('settings.advanced.gcOptions.zTitle'), subtitle: t('settings.advanced.gcOptions.zSubtitle') },
]);

const gcArgsPreview = computed(() => {
  switch (options.value.gcStrategy) {
    case 'g1':
      return '-XX:+UnlockExperimentalVMOptions -XX:+UseG1GC -XX:G1HeapRegionSize=32M -XX:G1NewSizePercent=20 -XX:G1ReservePercent=20 -XX:MaxGCPauseMillis=50';
    case 'parallel':
      return '-XX:+UseParallelGC';
    case 'serial':
      return '-XX:+UseSerialGC';
    case 'z':
      return t('settings.advanced.gcArgs.z');
    default:
      return t('settings.advanced.gcArgs.default');
  }
});

async function loadOptions() {
  try {
    options.value = await configApi.getJvmAdvancedOptions();
    metaspaceEnabled.value = metaspaceConfigured(options.value);
    isInitialized.value = true;
    if (showPreview.value) await refreshPreview();
  } catch (err) {
    logError('Failed to load JVM advanced options', err, 'AdvancedSettings');
  }
}

async function saveOptions() {
  saving.value = true;
  try {
    await configApi.setJvmAdvancedOptions(options.value);
    await refreshPreview();
  } catch (err) {
    logError('Failed to save JVM advanced options', err, 'AdvancedSettings');
  } finally {
    saving.value = false;
  }
}

async function refreshPreview() {
  try {
    effectiveArgs.value = await configApi.getEffectiveJvmArgs(settingsStore.maxMemory);
  } catch (err) {
    logError('Failed to preview JVM args', err, 'AdvancedSettings');
    effectiveArgs.value = [];
  }
}

function metaspaceConfigured(opts: JvmAdvancedOptions): boolean {
  return opts.metaspaceSizeMb !== null || opts.metaspaceMaxSizeMb !== null;
}

function toggleMetaspace(value: boolean | null) {
  const enabled = value === true;
  metaspaceEnabled.value = enabled;
  if (enabled) {
    options.value.metaspaceSizeMb = options.value.metaspaceSizeMb ?? 256;
    options.value.metaspaceMaxSizeMb = options.value.metaspaceMaxSizeMb ?? 512;
  } else {
    options.value.metaspaceSizeMb = null;
    options.value.metaspaceMaxSizeMb = null;
  }
}

function resetToDefaults() {
  options.value = {
    gcStrategy: 'g1',
    alwaysPreTouch: false,
    disableExplicitGc: false,
    stringDeduplication: false,
    perfDisableSharedMem: false,
    metaspaceSizeMb: null,
    metaspaceMaxSizeMb: null,
    heapDumpOnOom: false,
    extraArgs: '',
  };
  metaspaceEnabled.value = false;
}

// extraArgs 是逐字符输入的 textarea：不防抖的话每敲一个字就是两次 IPC（保存 + 预览刷新）
const debouncedSaveOptions = useDebounceFn(() => {
  void saveOptions();
}, 500);

watch(
  options,
  () => {
    if (!isInitialized.value) return;
    debouncedSaveOptions.call();
  },
  { deep: true },
);

watch(
  () => [options.value.metaspaceSizeMb, options.value.metaspaceMaxSizeMb],
  () => {
    if (!isInitialized.value) return;
    metaspaceEnabled.value = metaspaceConfigured(options.value);
  },
);

onMounted(async () => {
  await settingsStore.loadSettings();
  await loadOptions();
});
</script>

<template>
  <div class="settings-group">
    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-1">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-recycle</v-icon>
          <span class="text-title-medium">{{ t('settings.advanced.gcTitle') }}</span>
        </div>
        <p class="text-body-medium text-on-surface-variant mb-4">{{ t('settings.advanced.gcHint') }}</p>

        <v-select
          v-model="options.gcStrategy"
          :items="gcOptions"
          item-title="title"
          item-value="value"
          :label="t('settings.advanced.gcStrategyLabel')"
          density="comfortable"
          variant="outlined"
          hide-details
        />

        <p class="text-body-medium text-on-surface-variant mt-4 mb-0">
          {{ t('settings.advanced.gcArgsPreview') }}
        </p>
        <pre class="jvm-code mt-2">{{ gcArgsPreview }}</pre>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-3">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-tune-variant</v-icon>
          <span class="text-title-medium">{{ t('settings.advanced.optionsTitle') }}</span>
        </div>
        <p class="text-body-medium text-on-surface-variant mb-4">
          {{ t('settings.advanced.optionsHint') }}
        </p>

        <div class="option-row">
          <div class="option-text">
            <span class="text-body-medium">{{ t('settings.advanced.alwaysPreTouch') }}</span>
            <p class="text-body-small text-on-surface-variant mb-0">
              {{ t('settings.advanced.alwaysPreTouchHint') }}
            </p>
          </div>
          <v-switch
            v-model="options.alwaysPreTouch"
            :aria-label="t('settings.advanced.alwaysPreTouchAria')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <v-divider class="my-3" />

        <div class="option-row">
          <div class="option-text">
            <span class="text-body-medium">{{ t('settings.advanced.disableExplicitGc') }}</span>
            <p class="text-body-small text-on-surface-variant mb-0">
              {{ t('settings.advanced.disableExplicitGcHint') }}
            </p>
          </div>
          <v-switch
            v-model="options.disableExplicitGc"
            :aria-label="t('settings.advanced.disableExplicitGcAria')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <v-divider class="my-3" />

        <div class="option-row">
          <div class="option-text">
            <span class="text-body-medium">{{ t('settings.advanced.stringDedup') }}</span>
            <p class="text-body-small text-on-surface-variant mb-0">{{ t('settings.advanced.stringDedupHint') }}</p>
          </div>
          <v-switch
            v-model="options.stringDeduplication"
            :aria-label="t('settings.advanced.stringDedupAria')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <v-divider class="my-3" />

        <div class="option-row">
          <div class="option-text">
            <span class="text-body-medium">{{ t('settings.advanced.perfDisableSharedMem') }}</span>
            <p class="text-body-small text-on-surface-variant mb-0">
              {{ t('settings.advanced.perfDisableSharedMemHint') }}
            </p>
          </div>
          <v-switch
            v-model="options.perfDisableSharedMem"
            :aria-label="t('settings.advanced.perfDisableSharedMemAria')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>

        <v-divider class="my-3" />

        <div class="option-row">
          <div class="option-text">
            <span class="text-body-medium">{{ t('settings.advanced.heapDump') }}</span>
            <p class="text-body-small text-on-surface-variant mb-0">
              {{ t('settings.advanced.heapDumpHint') }}
            </p>
          </div>
          <v-switch
            v-model="options.heapDumpOnOom"
            :aria-label="t('settings.advanced.heapDumpAria')"
            hide-details
            inset
            density="compact"
            color="primary"
          />
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-1">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-memory</v-icon>
            <span class="text-title-medium">{{ t('settings.advanced.metaspaceTitle') }}</span>
          </div>
          <v-switch
            :model-value="metaspaceEnabled"
            :aria-label="t('settings.advanced.metaspaceToggleAria')"
            hide-details
            inset
            density="compact"
            color="primary"
            @update:model-value="toggleMetaspace"
          />
        </div>
        <p class="text-body-medium text-on-surface-variant mb-0">
          {{ t('settings.advanced.metaspaceHint') }}
        </p>

        <v-expand-transition>
          <div v-if="metaspaceEnabled" class="mt-4">
            <div class="text-body-medium mb-2">{{ t('settings.advanced.metaspaceInitial') }}</div>
            <div class="d-flex flex-wrap ga-2 mb-3">
              <v-chip
                v-for="mb in metaspacePresets"
                :key="mb"
                :variant="options.metaspaceSizeMb === mb ? 'flat' : 'tonal'"
                :color="options.metaspaceSizeMb === mb ? 'primary' : 'secondary'"
                @click="options.metaspaceSizeMb = mb"
              >
                {{ mb }} MB
              </v-chip>
            </div>
            <v-text-field
              v-model.number="options.metaspaceSizeMb"
              type="number"
              :label="t('settings.advanced.metaspaceInitialLabel')"
              suffix="MB"
              density="compact"
              hide-details
              :min="0"
              :hint="t('settings.advanced.metaspaceInitialHint')"
              style="max-width: 220px"
            />

            <v-divider class="my-4" />

            <div class="text-body-medium mb-2">{{ t('settings.advanced.metaspaceMax') }}</div>
            <div class="d-flex flex-wrap ga-2 mb-3">
              <v-chip
                v-for="mb in metaspaceMaxPresets"
                :key="mb"
                :variant="options.metaspaceMaxSizeMb === mb ? 'flat' : 'tonal'"
                :color="options.metaspaceMaxSizeMb === mb ? 'primary' : 'secondary'"
                @click="options.metaspaceMaxSizeMb = mb"
              >
                {{ mb }} MB
              </v-chip>
            </div>
            <v-text-field
              v-model.number="options.metaspaceMaxSizeMb"
              type="number"
              :label="t('settings.advanced.metaspaceMaxLabel')"
              suffix="MB"
              density="compact"
              hide-details
              :min="0"
              :hint="t('settings.advanced.metaspaceMaxHint')"
              style="max-width: 220px"
            />

            <v-alert v-if="metaspaceTooSmall" color="warning-container" density="compact" class="mt-4">
              <span class="text-body-small text-on-warning-container">
                {{ t('settings.advanced.metaspaceTooSmallPrefix') }}
                <code>OutOfMemoryError: Metaspace</code>
                {{ t('settings.advanced.metaspaceTooSmallSuffix') }}
              </span>
            </v-alert>
          </div>
        </v-expand-transition>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-1">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-console-line</v-icon>
          <span class="text-title-medium">{{ t('settings.advanced.extraArgsTitle') }}</span>
        </div>
        <p class="text-body-medium text-on-surface-variant mb-3">
          {{ t('settings.advanced.extraArgsHint') }} <code>#</code> {{ t('settings.advanced.extraArgsHintComment') }}
        </p>

        <v-textarea
          v-model="options.extraArgs"
          variant="outlined"
          density="compact"
          rows="5"
          hide-details
          auto-grow
          :disabled="saving"
          spellcheck="false"
          :placeholder="t('settings.advanced.extraArgsPlaceholder')"
        />
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-3">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-file-document-outline</v-icon>
            <span class="text-title-medium">{{ t('settings.advanced.effectiveTitle') }}</span>
          </div>
          <v-btn
            size="small"
            variant="text"
            color="primary"
            :loading="saving"
            @click="
              showPreview = !showPreview;
              refreshPreview();
            "
          >
            {{ showPreview ? t('settings.advanced.effectiveRefresh') : t('settings.advanced.effectivePreview') }}
          </v-btn>
        </div>

        <v-expand-transition>
          <div v-if="showPreview">
            <p v-if="settingsStore.autoMemoryEnabled" class="text-body-small text-on-surface-variant mb-2">
              {{ t('settings.advanced.autoMemoryNote', { memory: settingsStore.maxMemory }) }}
            </p>
            <pre v-if="effectiveArgs.length" class="jvm-code jvm-code-block">{{ effectiveArgs.join('\n') }}</pre>
            <p v-else class="text-body-medium text-on-surface-variant mb-0">{{ t('settings.advanced.noPreview') }}</p>
          </div>
        </v-expand-transition>
      </v-card-text>
    </v-card>

    <v-alert color="warning-container" density="compact" class="tip-alert mb-4">
      <template #prepend>
        <v-icon size="20" color="on-warning-container">mdi-alert-outline</v-icon>
      </template>
      <ul class="text-body-small pl-4 mb-0 text-on-warning-container">
        <li>{{ t('settings.advanced.tip1') }}</li>
        <li>{{ t('settings.advanced.tip2') }}</li>
      </ul>
    </v-alert>

    <v-btn variant="text" color="secondary" prepend-icon="mdi-restore" @click="resetToDefaults">
      {{ t('settings.advanced.reset') }}
    </v-btn>
  </div>
</template>

<style scoped>
.option-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.option-text {
  flex: 1 1 auto;
  min-width: 0;
}

.jvm-code {
  display: block;
  padding: 12px;
  border-radius: 8px;
  background: rgb(var(--v-theme-surface-container-high));
  font-family: monospace;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
}

.jvm-code-block {
  max-height: 240px;
  overflow-y: auto;
}
</style>
