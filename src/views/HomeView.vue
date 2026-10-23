<script setup lang="ts">
import { ref, onMounted, computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { VStepperVertical, VStepperVerticalItem } from 'vuetify/components';
import InstanceAvatar from '../components/instance/InstanceAvatar.vue';
import PlayerHead from '../components/PlayerHead.vue';
import { useVersionManager } from '../composables/useVersionManager';
import { useGameLaunch } from '../composables/useGameLaunch';
import { useAuthStore } from '../stores/authStore';
import { instanceApi } from '../services';
import { formatLastPlayed } from '../utils/format';
import { logError } from '../utils/logger';
import type { GameInstance } from '../types/events';

const { t } = useI18n();

const { selectedVersion: selectedInstance, loadGameDir, initListeners } = useVersionManager();

const { loading: launchLoading, launchInstance, isRepairing, repairProgress } = useGameLaunch();

const authStore = useAuthStore();
const pageLoading = ref(true);

const isReady = computed(() => {
  if (!selectedInstance.value) return false;
  return authStore.isLoggedIn;
});

const instances = ref<GameInstance[]>([]);
const instancesLoading = ref(false);
const instanceNames = computed(() => instances.value.map((i) => i.name));
const selectedInstanceData = computed<GameInstance | null>(
  () => instances.value.find((i) => i.name === selectedInstance.value) ?? null,
);
const recentPlays = computed(() => {
  return [...instances.value]
    .filter((i) => i.lastPlayed != null)
    .sort((a, b) => (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0))
    .slice(0, 5);
});

const instanceSummary = computed(() => {
  const i = selectedInstanceData.value;
  if (!i) return t('home.selectInstanceHint');
  const loader =
    i.loaderType && i.loaderType !== 'None' && i.loaderType !== 'Modded'
      ? i.loaderVersion
        ? `${i.loaderType} ${i.loaderVersion}`
        : i.loaderType
      : '';
  const memory = i.autoMemoryMb ? t('home.autoMemory', { size: (i.autoMemoryMb / 1024).toFixed(1) }) : '';
  // 实例名经常就是版本号本身，副标题再写一遍只会变成噪音
  const parts = [
    i.gameVersion || i.version,
    loader,
    i.modCount ? t('home.modCount', { count: i.modCount }, i.modCount) : '',
    memory,
  ]
    .filter(Boolean)
    .filter((p) => p !== i.name);
  if (parts.length === 0 && i.lastPlayed != null) {
    parts.push(t('home.lastPlayed', { time: formatLastPlayed(i.lastPlayed) }));
  }
  return parts.join(' · ');
});

const accountLabel = computed(() => {
  if (authStore.authType === 'microsoft') return authStore.msLoggedIn ? authStore.msUsername : t('home.notLoggedIn');
  return authStore.username || t('home.noPlayerName');
});

function quickLaunch(instanceName: string) {
  selectedInstance.value = instanceName;
  if (isReady.value) {
    handleLaunch();
  }
}

async function loadInstances() {
  instancesLoading.value = true;
  try {
    const list = await instanceApi.getInstances();
    instances.value = list || [];
    if (!instanceNames.value.includes(selectedInstance.value)) {
      selectedInstance.value = instanceNames.value[0] ?? '';
    }
  } catch (e) {
    logError('Failed to load instances', e, 'HomeView');
  } finally {
    instancesLoading.value = false;
  }
}

async function handleLaunch() {
  const launched = await launchInstance(selectedInstance.value);
  if (launched) {
    await loadInstances();
  }
}

onMounted(async () => {
  await Promise.all([loadGameDir(), authStore.init(), initListeners()]);
  pageLoading.value = false;
  await loadInstances();
});
</script>

<template>
  <v-container fluid class="page-container pa-4">
    <template v-if="pageLoading">
      <v-card color="surface-container" variant="flat" class="mb-4">
        <v-card-text class="pa-5">
          <div class="d-flex align-center flex-wrap ga-5">
            <v-skeleton-loader type="avatar" />
            <div class="hero-id">
              <v-skeleton-loader type="heading" boilerplate style="width: 180px" />
              <v-skeleton-loader type="text" boilerplate style="width: 240px" class="mt-1" />
              <v-skeleton-loader type="text" boilerplate style="width: 140px" />
            </div>
            <div class="hero-actions">
              <v-skeleton-loader type="text" class="flex-grow-1" style="height: 40px" />
              <v-skeleton-loader type="button" style="width: 200px; height: 52px" />
            </div>
          </div>
        </v-card-text>
      </v-card>

      <v-row>
        <v-col cols="12" md="7">
          <v-card color="surface-container" variant="flat" class="h-100">
            <v-card-text class="pa-4">
              <v-skeleton-loader type="list-item-two-line" />
              <v-skeleton-loader type="list-item-two-line" />
              <v-skeleton-loader type="list-item-two-line" />
            </v-card-text>
          </v-card>
        </v-col>
        <v-col cols="12" md="5">
          <v-card color="surface-container" variant="flat" class="h-100">
            <v-card-text class="pa-4">
              <v-skeleton-loader type="text" style="width: 100px" class="mb-3" />
              <v-skeleton-loader type="button" style="width: 100%" class="mb-2" />
              <v-skeleton-loader type="button" style="width: 100%" />
            </v-card-text>
          </v-card>
        </v-col>
      </v-row>
    </template>

    <template v-else>
      <!-- 启动区卡片按内容高度，不要加 h-100 -->
      <v-card color="surface-container" variant="flat" class="mb-4">
        <v-card-text class="pa-5">
          <div class="d-flex align-center flex-wrap ga-5">
            <InstanceAvatar v-if="selectedInstanceData" :instance="selectedInstanceData" :size="72" />
            <v-avatar v-else size="72" color="primary">
              <v-icon size="36" color="on-primary">mdi-minecraft</v-icon>
            </v-avatar>

            <div class="hero-id">
              <div class="text-headline-small text-truncate">
                {{ selectedInstance || t('home.noInstanceSelected') }}
              </div>
              <div class="text-body-medium text-on-surface-variant text-truncate">{{ instanceSummary }}</div>
              <div class="d-flex align-center ga-2 mt-1">
                <PlayerHead v-if="authStore.msSkinUrl" :skin-url="authStore.msSkinUrl" :size="20" />
                <v-icon v-else size="18" :color="isReady ? 'on-surface-variant' : 'warning'">
                  {{ authStore.authType === 'microsoft' ? 'mdi-microsoft' : 'mdi-account' }}
                </v-icon>
                <span class="text-body-small font-weight-medium">{{ accountLabel }}</span>
                <v-btn size="x-small" variant="text" to="/settings">{{ t('home.account') }}</v-btn>
              </div>
            </div>

            <!-- 操作栏选择器和按钮并排，启动按钮不要拉成整栏宽 -->
            <div class="hero-actions">
              <v-select
                v-model="selectedInstance"
                :items="instanceNames"
                :loading="instancesLoading"
                :label="t('home.instanceLabel')"
                density="comfortable"
                variant="outlined"
                hide-details
                class="flex-grow-1"
                style="min-width: 200px"
              >
                <template #append-inner>
                  <v-btn
                    icon
                    variant="text"
                    size="x-small"
                    :aria-label="t('home.reloadInstances')"
                    :loading="instancesLoading"
                    @click.stop="loadInstances"
                  >
                    <v-icon size="16">mdi-refresh</v-icon>
                  </v-btn>
                </template>
                <template #no-data>
                  <v-list-item>
                    <v-list-item-title class="text-on-surface-variant">{{
                      t('home.noInstancesYet')
                    }}</v-list-item-title>
                  </v-list-item>
                </template>
              </v-select>

              <v-btn
                size="x-large"
                :color="isReady ? 'primary' : 'surface-container-high'"
                :loading="launchLoading"
                :disabled="!isReady"
                class="launch-btn"
                style="min-width: 200px"
                @click="handleLaunch"
              >
                <v-icon start size="24">mdi-play</v-icon>
                {{ t('home.launchGame') }}
              </v-btn>
            </div>
          </div>

          <div v-if="!isReady" class="launch-guide mt-4">
            <v-stepper-vertical hide-actions flat>
              <v-stepper-vertical-item
                :complete="!!selectedInstance"
                :color="selectedInstance ? 'success' : 'warning'"
                :icon="selectedInstance ? 'mdi-check' : 'mdi-cube-outline'"
                :title="t('home.stepSelectInstance')"
                value="version"
              >
                <div class="text-body-small text-on-surface-variant">{{ t('home.stepSelectInstanceHint') }}</div>
              </v-stepper-vertical-item>
              <v-stepper-vertical-item
                :complete="!!authStore.displayName"
                :color="authStore.displayName ? 'success' : 'warning'"
                :icon="authStore.displayName ? 'mdi-check' : 'mdi-account-outline'"
                :title="authStore.authType === 'microsoft' ? t('home.stepLoginMicrosoft') : t('home.stepSetPlayerName')"
                value="auth"
              >
                <div class="text-body-small text-on-surface-variant">
                  <i18n-t keypath="home.stepAuthHint" tag="span">
                    <template #link>
                      <router-link to="/settings" class="text-primary">{{ t('home.settingsLink') }}</router-link>
                    </template>
                  </i18n-t>
                </div>
              </v-stepper-vertical-item>
            </v-stepper-vertical>
          </div>
        </v-card-text>
      </v-card>

      <v-row>
        <v-col cols="12" md="7">
          <div class="d-flex flex-column h-100">
            <v-card color="surface-container" variant="flat" class="d-flex flex-column flex-grow-1">
              <v-card-text class="pa-4 d-flex align-center card-head-row">
                <div class="d-flex align-center flex-grow-1">
                  <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-history</v-icon>
                  <span class="text-title-medium">{{ t('home.recentPlays') }}</span>
                </div>
                <span class="text-body-small text-on-surface-variant mr-3">
                  {{ t('home.instanceCount', { count: instances.length }, instances.length) }}
                </span>
                <v-btn size="x-small" variant="text" to="/instance-manager">{{ t('home.instanceListLink') }}</v-btn>
              </v-card-text>
              <v-divider />
              <v-card-text class="pa-4 flex-grow-1">
                <div v-if="recentPlays.length === 0" class="text-body-small text-on-surface-variant">
                  {{ t('home.noRecentPlays') }}
                </div>
                <v-list v-else density="compact" bg-color="transparent">
                  <v-list-item v-for="instance in recentPlays" :key="instance.id" class="px-0">
                    <template #prepend>
                      <InstanceAvatar :instance="instance" :size="32" class="mr-2" />
                    </template>
                    <v-list-item-title class="text-body-medium">
                      {{ instance.name }}
                    </v-list-item-title>
                    <v-list-item-subtitle class="text-body-small">
                      {{ instance.gameVersion || instance.version
                      }}<template
                        v-if="instance.loaderType && instance.loaderType !== 'None' && instance.loaderType !== 'Modded'"
                      >
                        - {{ instance.loaderType
                        }}<template v-if="instance.loaderVersion">{{
                          ' ' + instance.loaderVersion
                        }}</template></template
                      >
                      · {{ formatLastPlayed(instance.lastPlayed) }}
                    </v-list-item-subtitle>
                    <template #append>
                      <div class="d-flex align-center">
                        <v-btn
                          size="x-small"
                          variant="text"
                          color="primary"
                          :aria-label="`${t('home.launch')} ${instance.name}`"
                          @click="quickLaunch(instance.name)"
                        >
                          {{ t('home.launch') }}
                        </v-btn>
                        <v-btn
                          size="x-small"
                          variant="text"
                          :aria-label="`${t('home.details')} ${instance.name}`"
                          :to="{ name: 'instance-detail', params: { name: instance.name } }"
                        >
                          {{ t('home.details') }}
                        </v-btn>
                      </div>
                    </template>
                  </v-list-item>
                </v-list>
              </v-card-text>
            </v-card>
          </div>
        </v-col>

        <v-col cols="12" md="5">
          <div class="d-flex flex-column h-100">
            <v-card color="surface-container" variant="flat">
              <v-card-text class="pa-4">
                <div class="d-flex align-center mb-3">
                  <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-lightning-bolt</v-icon>
                  <span class="text-title-medium">{{ t('home.quickActions') }}</span>
                </div>
                <div class="d-flex flex-column ga-2">
                  <v-btn variant="tonal" color="primary" block to="/download">
                    <v-icon start size="18">mdi-download</v-icon>
                    {{ t('home.downloadVersion') }}
                  </v-btn>
                  <v-btn variant="tonal" color="secondary" block to="/add-instance">
                    <v-icon start size="18">mdi-plus</v-icon>
                    {{ t('home.createInstance') }}
                  </v-btn>
                </div>
              </v-card-text>
            </v-card>
          </div>
        </v-col>
      </v-row>
    </template>

    <v-dialog :model-value="isRepairing" persistent max-width="380">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-center mb-4">
            <v-progress-circular v-if="!repairProgress" indeterminate size="44" color="primary" />
            <v-avatar v-else size="52" color="primary-container">
              <v-icon size="26" color="on-primary-container">mdi-wrench</v-icon>
            </v-avatar>
          </div>
          <div class="text-title-large font-weight-bold text-center mb-4">{{ t('home.repairing') }}</div>

          <template v-if="repairProgress">
            <div class="d-flex justify-space-between mb-2 text-body-medium num-mono">
              <span>{{ repairProgress.status === 'downloading' ? t('home.downloading') : t('home.processing') }}</span>
              <span class="font-weight-medium">{{ repairProgress.percent }}%</span>
            </div>
            <v-progress-linear :model-value="repairProgress.percent" height="6" rounded color="primary" />
            <div class="d-flex justify-space-between mt-2 text-body-small text-on-surface-variant num-mono">
              <span>{{ (repairProgress.bytes_downloaded / 1024 / 1024).toFixed(1) }} MB</span>
              <span>{{
                repairProgress.speed < 1024
                  ? repairProgress.speed.toFixed(0) + ' KB/s'
                  : (repairProgress.speed / 1024).toFixed(1) + ' MB/s'
              }}</span>
            </div>
          </template>
          <div v-else class="text-center text-body-medium text-on-surface-variant">{{ t('home.preparing') }}</div>
        </v-card-text>
      </v-card>
    </v-dialog>
  </v-container>
</template>

<style scoped>
.launch-btn {
  font-weight: 700;
  font-size: 1.1rem;
  letter-spacing: 0.5px;
  height: 52px !important;
}

.launch-btn:not(:disabled) {
  box-shadow: 0 4px 16px rgba(var(--v-theme-primary), 0.35);
}

.hero-id {
  flex: 1 1 240px;
  min-width: 0;
}

.hero-actions {
  flex: 1 1 440px;
  display: flex;
  align-items: center;
  gap: 12px;
}

.launch-guide :deep(.v-stepper-vertical) {
  background: transparent;
  padding: 0;
}

.launch-guide :deep(.v-stepper-vertical-item__title) {
  font-size: 0.875rem;
}
</style>
