<script setup lang="ts">
/**
 * 实例详情页
 * 从实例列表进入，按标签组织这个实例的模组、游戏设置、资源包与光影
 */
import { computed, onMounted, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import InstanceAvatar from '../components/instance/InstanceAvatar.vue';
import InstanceSettingsPanel from '../components/instance/InstanceSettingsPanel.vue';
import LogsPanel from '../components/instance/LogsPanel.vue';
import ModsPanel from '../components/instance/ModsPanel.vue';
import ResourcePacksPanel from '../components/instance/ResourcePacksPanel.vue';
import SavesPanel from '../components/instance/SavesPanel.vue';
import ShaderPacksPanel from '../components/instance/ShaderPacksPanel.vue';
import ExportModpackDialog from '../components/instance/ExportModpackDialog.vue';
import { useNotificationStore } from '../stores/notificationStore';
import { getInstanceListCache } from '../stores/instanceListCache';
import { instanceApi } from '../services';
import { useGameLaunch } from '../composables/useGameLaunch';
import type { GameInstance } from '../types/events';
import { formatLastPlayed } from '../utils/format';
import { logError } from '../utils/logger';

const route = useRoute();
const router = useRouter();
const notificationStore = useNotificationStore();
const { launchInstance: startInstance } = useGameLaunch();
const { t } = useI18n();

const tab = ref<'mods' | 'settings' | 'resourcepacks' | 'shaderpacks' | 'saves' | 'logs'>('settings');

const instanceName = computed(() => String(route.params.name ?? ''));

// 列表页已经拉过一次，先用缓存把头部画出来，再静默刷新，避免整页转圈闪一下
const cachedInstance = getInstanceListCache().find((item) => item.name === instanceName.value) ?? null;
const instance = ref<GameInstance | null>(cachedInstance);
const loading = ref(cachedInstance === null);
const error = ref('');
const launching = ref(false);
const exportDialog = ref(false);

const versionLabel = computed(() => {
  const value = instance.value;
  if (!value) return '';
  const mc = value.gameVersion || value.version;
  const loader = value.loaderType && value.loaderType !== 'None' ? value.loaderType : '';
  return loader ? `${mc} · ${loader}` : mc;
});

/**
 * silent 用于模组操作后的刷新：不切加载态，避免把整个页面（连同模组面板的更新列表）重建一遍
 */
async function loadInstance(silent = false) {
  if (!silent) {
    loading.value = true;
  }
  error.value = '';
  try {
    const instances = await instanceApi.getInstances();
    instance.value = instances.find((item) => item.name === instanceName.value) ?? null;
    if (!instance.value) {
      error.value = t('instanceDetail.notFound', { name: instanceName.value });
    }
  } catch (e) {
    logError('Failed to load instance detail', e, 'InstanceDetailView');
    error.value = t('instanceDetail.loadFailed');
  } finally {
    if (!silent) {
      loading.value = false;
    }
  }
}

async function launch() {
  if (!instance.value) return;

  launching.value = true;
  try {
    if (await startInstance(instance.value.name)) {
      await loadInstance(true);
    }
  } finally {
    launching.value = false;
  }
}

async function openFolder() {
  if (!instance.value) return;
  try {
    await instanceApi.openInstanceFolder(instance.value.name);
  } catch (e) {
    logError('Failed to open instance folder', e, 'InstanceDetailView');
    notificationStore.error(t('instanceDetail.openFolderFailed'), t('instanceDetail.openFolderFailedHint'));
  }
}

function goBack() {
  router.push({ name: 'instance-manager' });
}

watch(instanceName, () => {
  const cached = getInstanceListCache().find((item) => item.name === instanceName.value) ?? null;
  instance.value = cached;
  loading.value = cached === null;
  void loadInstance(cached !== null);
});

onMounted(() => {
  void loadInstance(cachedInstance !== null);
});
</script>

<template>
  <v-container fluid class="page-container pa-4">
    <div class="d-flex align-center mb-3">
      <v-btn variant="text" prepend-icon="mdi-arrow-left" @click="goBack">{{ t('instanceDetail.back') }}</v-btn>
    </div>

    <v-card v-if="loading" variant="flat" color="surface-container" class="pa-8">
      <div class="d-flex align-center justify-center">
        <v-progress-circular indeterminate color="primary" />
      </div>
    </v-card>

    <v-card v-else-if="error" variant="flat" color="surface-container" class="pa-8 text-center">
      <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-alert-circle-outline</v-icon>
      <div class="text-body-large mb-4">{{ error }}</div>
      <v-btn color="primary" variant="tonal" @click="goBack">{{ t('instanceDetail.back') }}</v-btn>
    </v-card>

    <template v-else-if="instance">
      <v-card variant="flat" color="surface-container" class="mb-3">
        <v-card-text class="pa-4 d-flex align-center flex-wrap ga-3">
          <InstanceAvatar :instance="instance" :size="56" />

          <div class="flex-grow-1 min-width-0">
            <div class="text-title-large font-weight-bold text-truncate">{{ instance.name }}</div>
            <div class="text-body-small text-on-surface-variant">
              {{ versionLabel }}
              <template v-if="instance.modCount">
                · {{ t('loader.modsSuffix', { count: instance.modCount }, instance.modCount) }}
              </template>
              <template v-if="instance.lastPlayed"> · {{ formatLastPlayed(instance.lastPlayed) }}</template>
            </div>
          </div>

          <div class="d-flex align-center ga-2">
            <v-btn variant="text" prepend-icon="mdi-folder-open-outline" @click="openFolder">
              {{ t('instanceDetail.openFolder') }}
            </v-btn>
            <v-btn color="primary" :loading="launching" @click="launch">
              <v-icon start>mdi-play</v-icon>
              {{ t('instanceDetail.launch') }}
            </v-btn>
          </div>
        </v-card-text>
      </v-card>

      <v-tabs v-model="tab" color="primary" class="mb-3">
        <v-tab value="settings">
          <v-icon start size="18">mdi-cog-outline</v-icon>
          {{ t('instanceDetail.tabSettings') }}
        </v-tab>
        <v-tab value="mods">
          <v-icon start size="18">mdi-puzzle-outline</v-icon>
          {{ t('instanceDetail.tabMods') }}
        </v-tab>
        <v-tab value="resourcepacks">
          <v-icon start size="18">mdi-texture-box</v-icon>
          {{ t('instanceDetail.tabResourcePacks') }}
        </v-tab>
        <v-tab value="shaderpacks">
          <v-icon start size="18">mdi-shimmer</v-icon>
          {{ t('instanceDetail.tabShaderPacks') }}
        </v-tab>
        <v-tab value="saves">
          <v-icon start size="18">mdi-content-save-outline</v-icon>
          {{ t('instanceDetail.tabSaves') }}
        </v-tab>
        <v-tab value="logs">
          <v-icon start size="18">mdi-text-box-search-outline</v-icon>
          {{ t('instanceDetail.tabLogs') }}
        </v-tab>
      </v-tabs>

      <v-window v-model="tab">
        <v-window-item value="mods">
          <ModsPanel
            :instance-name="instance.name"
            :game-version="instance.gameVersion"
            :loader="instance.loaderType"
            @changed="loadInstance(true)"
          />
        </v-window-item>

        <v-window-item value="settings">
          <InstanceSettingsPanel :instance-name="instance.name" />

          <v-card variant="flat" color="surface-container">
            <v-card-text class="pa-4">
              <div class="text-title-small mb-3">{{ t('instanceDetail.instanceInfo') }}</div>
              <v-row density="compact">
                <v-col cols="12" sm="6" md="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('instanceDetail.minecraftVersion') }}</div>
                  <div class="text-body-medium">{{ instance.gameVersion || instance.version }}</div>
                </v-col>
                <v-col cols="12" sm="6" md="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('instanceDetail.loader') }}</div>
                  <div class="text-body-medium">
                    {{
                      instance.loaderType && instance.loaderType !== 'None'
                        ? instance.loaderType
                        : t('instanceDetail.vanilla')
                    }}
                    <template v-if="instance.loaderVersion"> {{ instance.loaderVersion }}</template>
                  </div>
                </v-col>
                <v-col cols="12" sm="6" md="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('instanceDetail.modCount') }}</div>
                  <div class="text-body-medium">{{ instance.modCount ?? 0 }}</div>
                </v-col>
                <v-col cols="12" sm="6" md="3">
                  <div class="text-body-small text-on-surface-variant">{{ t('instanceDetail.lastLaunch') }}</div>
                  <div class="text-body-medium">{{ formatLastPlayed(instance.lastPlayed) }}</div>
                </v-col>
              </v-row>

              <v-divider class="my-4" />

              <div class="d-flex ga-2 mt-4">
                <v-btn variant="tonal" prepend-icon="mdi-folder-open-outline" @click="openFolder">
                  {{ t('instanceDetail.openInstanceFolder') }}
                </v-btn>
                <v-btn variant="tonal" prepend-icon="mdi-package-variant-closed" @click="exportDialog = true">
                  {{ t('instanceManager.exportModpack') }}
                </v-btn>
                <v-btn variant="text" prepend-icon="mdi-cog-outline" to="/settings">
                  {{ t('instanceDetail.openGlobalSettings') }}
                </v-btn>
              </div>
            </v-card-text>
          </v-card>
        </v-window-item>

        <v-window-item value="resourcepacks">
          <ResourcePacksPanel
            :instance-name="instance.name"
            :game-version="instance.gameVersion"
            @changed="loadInstance(true)"
          />
        </v-window-item>

        <v-window-item value="shaderpacks">
          <ShaderPacksPanel
            :instance-name="instance.name"
            :game-version="instance.gameVersion"
            @changed="loadInstance(true)"
          />
        </v-window-item>

        <v-window-item value="saves">
          <SavesPanel :instance-name="instance.name" />
        </v-window-item>

        <v-window-item value="logs">
          <LogsPanel :instance-name="instance.name" />
        </v-window-item>
      </v-window>
    </template>

    <ExportModpackDialog v-model="exportDialog" :instance="instance" />
  </v-container>
</template>

<style scoped>
.min-width-0 {
  min-width: 0;
}
</style>
