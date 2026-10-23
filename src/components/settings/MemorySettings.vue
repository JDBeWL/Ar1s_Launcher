<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { useSettingsStore } from '../../stores/settings';
import { configApi } from '../../services';
import { logError } from '../../utils/logger';
import { watchDebounced } from '../../composables/useDebounce';

const settingsStore = useSettingsStore();
const { t } = useI18n();
const memoryWarning = ref('');

const autoMemoryEnabled = computed(() => settingsStore.autoMemoryEnabled);
const totalMemoryGB = computed(() => (settingsStore.totalMemoryMB / 1024).toFixed(1));
const usedMemoryGB = computed(() => (settingsStore.usedMemoryMB / 1024).toFixed(1));
const availableMemoryGB = computed(() => (settingsStore.availableMemoryMB / 1024).toFixed(1));
const maxMemoryGB = computed(() => (settingsStore.maxMemory / 1024).toFixed(1));
const memoryPercentage = computed(() =>
  settingsStore.totalMemoryMB > 0 ? Math.round((settingsStore.maxMemory / settingsStore.totalMemoryMB) * 100) : 0,
);
const projectedUsedMB = computed(() => settingsStore.usedMemoryMB + settingsStore.maxMemory);
const projectedUsedGB = computed(() => (projectedUsedMB.value / 1024).toFixed(1));
const projectedPercent = computed(() =>
  settingsStore.totalMemoryMB > 0 ? Math.round((projectedUsedMB.value / settingsStore.totalMemoryMB) * 100) : 0,
);
const projectedBarPercent = computed(() => Math.min(100, Math.max(0, projectedPercent.value)));
const projectedColor = computed(() => {
  if (projectedPercent.value > 100) return 'error';
  if (projectedPercent.value >= 90) return 'warning';
  return 'primary';
});
const projectedWarning = computed(() => {
  if (settingsStore.totalMemoryMB <= 0) return '';
  if (projectedPercent.value > 100) return t('settings.memory.warningOverLimit');
  if (projectedPercent.value >= 90) return t('settings.memory.warningNearLimit');
  return '';
});

// 手动模式上限就是物理内存总量，分配过高的风险由「预计占用」提示承担
const maxAllocatableMB = computed(() => Math.max(512, settingsStore.totalMemoryMB));

function clampMaxMemory() {
  const raw = Number(settingsStore.maxMemory);
  const clamped = Number.isFinite(raw) ? Math.min(Math.max(Math.round(raw), 512), maxAllocatableMB.value) : 512;
  settingsStore.maxMemory = clamped;
  settingsStore.saveMaxMemory();
}

async function checkMemoryWarning() {
  if (settingsStore.autoMemoryEnabled) {
    memoryWarning.value = '';
    return;
  }
  try {
    const warning = await configApi.checkMemoryWarning(settingsStore.maxMemory);
    memoryWarning.value = warning || '';
  } catch (err) {
    logError('Failed to check memory warning', err, 'MemorySettings');
    memoryWarning.value = '';
  }
}

async function toggleAutoMemory(value: boolean | null) {
  settingsStore.autoMemoryEnabled = value === true;
  await settingsStore.saveAutoMemoryEnabled();
  if (settingsStore.autoMemoryEnabled) {
    memoryWarning.value = '';
  } else {
    await checkMemoryWarning();
  }
}

function setPresetMemory(mb: number) {
  settingsStore.maxMemory = Math.min(mb, maxAllocatableMB.value);
  settingsStore.saveMaxMemory();
}

// 滑块拖动时每跨一档就会触发，不防抖的话一次拖动打出几十个 IPC
watchDebounced(
  () => settingsStore.maxMemory,
  () => {
    if (!settingsStore.autoMemoryEnabled) void checkMemoryWarning();
  },
);

let refreshTimer: ReturnType<typeof setInterval> | undefined;

onMounted(async () => {
  await settingsStore.loadSystemMemory();
  await settingsStore.loadSettings();
  await checkMemoryWarning();
  // 手动分配时占用会变，页面停留期间每 5 秒刷新一次实际占用
  refreshTimer = setInterval(() => void settingsStore.loadSystemMemory(), 5000);
});

onUnmounted(() => {
  if (refreshTimer !== undefined) clearInterval(refreshTimer);
});
</script>

<template>
  <div class="settings-group">
    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-chart-donut</v-icon>
          <span class="text-title-medium">{{ t('settings.memory.overviewTitle') }}</span>
        </div>

        <v-row align="center">
          <v-col cols="12" sm="4">
            <div class="memory-stat text-center pa-4">
              <div class="text-headline-medium font-weight-bold text-primary num-mono">{{ totalMemoryGB }}</div>
              <div class="text-body-medium text-on-surface-variant">{{ t('settings.memory.systemTotal') }}</div>
            </div>
          </v-col>
          <v-col cols="12" sm="4">
            <div class="memory-stat text-center pa-4">
              <div class="text-headline-medium font-weight-bold text-secondary num-mono">
                {{ autoMemoryEnabled ? t('settings.memory.autoLabel') : maxMemoryGB }}
              </div>
              <div class="text-body-medium text-on-surface-variant">
                {{ autoMemoryEnabled ? t('settings.memory.autoComputed') : t('settings.memory.allocated') }}
              </div>
            </div>
          </v-col>
          <v-col cols="12" sm="4">
            <div class="memory-stat text-center pa-4">
              <div class="text-headline-medium font-weight-bold text-info num-mono">
                {{ autoMemoryEnabled ? '—' : memoryPercentage + '%' }}
              </div>
              <div class="text-body-medium text-on-surface-variant">
                {{ autoMemoryEnabled ? t('settings.memory.autoComputedShort') : t('settings.memory.usageRatio') }}
              </div>
            </div>
          </v-col>
        </v-row>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-1">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-auto-fix</v-icon>
            <span class="text-title-medium">{{ t('settings.memory.autoTitle') }}</span>
          </div>
          <v-switch
            :model-value="autoMemoryEnabled"
            :aria-label="t('settings.memory.autoTitle')"
            hide-details
            inset
            density="compact"
            color="primary"
            @update:model-value="toggleAutoMemory"
          />
        </div>
        <p class="text-body-medium text-on-surface-variant mb-0">
          {{ t('settings.memory.autoHint') }}
        </p>

        <v-expand-transition>
          <div v-if="autoMemoryEnabled" class="mt-4">
            <v-alert color="primary-container" density="compact">
              <span class="text-on-primary-container text-body-medium">
                {{ t('settings.memory.autoDetail') }}
              </span>
            </v-alert>
          </div>
        </v-expand-transition>
      </v-card-text>
    </v-card>

    <v-card variant="flat" v-if="!autoMemoryEnabled" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-tune-vertical</v-icon>
          <span class="text-title-medium">{{ t('settings.memory.manualTitle') }}</span>
        </div>

        <div class="memory-usage mb-5">
          <div class="d-flex justify-space-between align-center text-body-small mb-1">
            <span class="text-on-surface-variant">{{ t('settings.memory.systemUsage') }}</span>
            <span class="num-mono">
              {{
                t('settings.memory.usageOf', {
                  used: usedMemoryGB,
                  total: totalMemoryGB,
                  percent: Math.round(settingsStore.memoryUsagePercent),
                })
              }}
            </span>
          </div>
          <div class="d-flex justify-space-between align-center text-body-small mb-1">
            <span class="text-on-surface-variant">{{ t('settings.memory.availableMemory') }}</span>
            <span class="num-mono">{{ availableMemoryGB }} GB</span>
          </div>
          <div class="d-flex justify-space-between align-center text-body-small mb-1">
            <span class="text-on-surface-variant">{{ t('settings.memory.gameUsage') }}</span>
            <span class="num-mono">{{ maxMemoryGB }} GB</span>
          </div>
          <div class="d-flex justify-space-between align-center text-body-small">
            <span class="text-on-surface-variant">{{ t('settings.memory.projectedUsage') }}</span>
            <span class="num-mono">
              {{
                t('settings.memory.usageOf', {
                  used: projectedUsedGB,
                  total: totalMemoryGB,
                  percent: projectedPercent,
                })
              }}
            </span>
          </div>
          <v-progress-linear
            :model-value="projectedBarPercent"
            :color="projectedColor"
            bg-color="surface-container-highest"
            height="8"
            rounded
            class="mt-3"
          />
          <div v-if="projectedWarning" class="text-body-small text-warning mt-2">{{ projectedWarning }}</div>
        </div>

        <div class="mb-5">
          <div class="text-body-medium text-on-surface-variant mb-3">{{ t('settings.memory.presetsTitle') }}</div>
          <div class="d-flex flex-wrap ga-2">
            <v-chip
              v-for="preset in [2048, 4096, 6144, 8192]"
              :key="preset"
              :variant="settingsStore.maxMemory === preset ? 'flat' : 'tonal'"
              :color="settingsStore.maxMemory === preset ? 'primary' : 'secondary'"
              :disabled="preset > maxAllocatableMB"
              @click="setPresetMemory(preset)"
            >
              {{ preset / 1024 }} GB
            </v-chip>
          </div>
        </div>

        <div class="mb-4">
          <div class="d-flex align-center justify-space-between mb-2">
            <span class="text-body-medium">{{ t('settings.memory.customMemory') }}</span>
            <v-text-field
              v-model.number="settingsStore.maxMemory"
              type="number"
              density="compact"
              :aria-label="t('settings.memory.customMemoryAria')"
              suffix="MB"
              hide-details
              hide-spin-buttons
              style="max-width: 140px"
              @change="clampMaxMemory"
            />
          </div>
          <v-slider
            v-model="settingsStore.maxMemory"
            :min="512"
            :max="maxAllocatableMB"
            :step="128"
            hide-details
            thumb-label
            color="primary"
            @end="settingsStore.saveMaxMemory"
          >
            <template #prepend>
              <span class="text-body-small text-on-surface-variant num-mono">512 MB</span>
            </template>
            <template #append>
              <span class="text-body-small text-on-surface-variant num-mono">{{ maxAllocatableMB }} MB</span>
            </template>
          </v-slider>
        </div>
      </v-card-text>
    </v-card>

    <v-alert v-if="memoryWarning" color="warning-container" density="compact" class="mb-4">
      <template #prepend>
        <v-icon color="on-warning-container">mdi-alert-outline</v-icon>
      </template>
      <span class="text-on-warning-container">{{ memoryWarning }}</span>
    </v-alert>

    <v-alert color="secondary-container" class="tip-alert">
      <template #prepend>
        <v-icon size="20" color="on-secondary-container">mdi-information-outline</v-icon>
      </template>
      <template #title>
        <span class="text-body-medium font-weight-medium text-on-secondary-container">
          {{ t('settings.memory.tipsTitle') }}
        </span>
      </template>
      <ul class="text-body-small pl-4 mb-0 mt-1 text-on-secondary-container">
        <li>{{ t('settings.memory.tip1') }}</li>
        <li>{{ t('settings.memory.tip2') }}</li>
        <li>{{ t('settings.memory.tip3') }}</li>
        <li>{{ t('settings.memory.tip4') }}</li>
      </ul>
    </v-alert>
  </div>
</template>

<style scoped>
.memory-stat {
  background: rgb(var(--v-theme-surface-container-high));
  border-radius: 16px;
}
</style>
