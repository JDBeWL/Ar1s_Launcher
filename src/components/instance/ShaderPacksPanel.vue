<script setup lang="ts">
/**
 * 实例详情页的光影包面板
 * 选中哪个包由游戏自己的配置决定（iris.properties / optionsshaders.txt），
 * 所以这里只改配置、不挪文件；同一时间只有一个包是启用的。
 */
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { open } from '@tauri-apps/plugin-dialog';
import ShaderPackBrowserDialog from './ShaderPackBrowserDialog.vue';
import { useNotificationStore } from '../../stores/notificationStore';
import { configApi, shaderPackApi } from '../../services';
import type { ShaderBackend, ShaderPackInfo, ShaderPackList, ShaderPackUpdate } from '../../types/events';
import { formatFileSize, getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const props = defineProps<{
  instanceName: string;
  gameVersion?: string | null;
}>();

const emit = defineEmits<{
  (e: 'changed'): void;
}>();

const notificationStore = useNotificationStore();
const { t } = useI18n();

const state = ref<ShaderPackList | null>(null);
const loading = ref(false);
const error = ref('');
const busyFile = ref<string | null>(null);
const toggling = ref(false);
const importing = ref(false);
const deleteDialog = ref(false);
const packToDelete = ref<ShaderPackInfo | null>(null);
const browser = ref(false);
const updates = ref<ShaderPackUpdate[]>([]);
const checking = ref(false);
const updatingFile = ref<string | null>(null);
const updatingAll = ref(false);
// 关闭版本隔离时 shaderpacks 放在共享目录，选择会影响所有实例
const versionIsolation = ref(true);

const packs = computed(() => state.value?.packs ?? []);
const selectedName = computed(() => state.value?.selected ?? null);
const backend = computed<ShaderBackend>(() => state.value?.backend ?? 'unknown');
const supportsToggle = computed(() => state.value?.supportsToggle ?? false);
const shadersEnabled = computed(() => state.value?.enabled ?? false);

const backendLabel = computed(() => {
  switch (backend.value) {
    case 'iris':
      return t('shaderPacks.backendIris');
    case 'oculus':
      return t('shaderPacks.backendOculus');
    case 'optifine':
      return t('shaderPacks.backendOptiFine');
    default:
      return t('shaderPacks.backendUnknown');
  }
});

const updateMap = computed(() => {
  const map = new Map<string, ShaderPackUpdate>();
  for (const update of updates.value) {
    map.set(update.fileName, update);
  }
  return map;
});

const updatableCount = computed(() => updates.value.length);

/** 配置里选中的包已经不在磁盘上：游戏会退回无光影，得在界面上说清楚 */
const selectedPack = computed(() => {
  const name = selectedName.value?.toLowerCase();
  if (!name) return null;
  // 配置里的名字大小写未必和磁盘一致，和后端的判定保持一致
  return packs.value.find((pack) => pack.fileName.toLowerCase() === name) ?? null;
});

const missingSelection = computed(() => (selectedName.value && !selectedPack.value ? selectedName.value : null));

const invalidCount = computed(() => packs.value.filter((pack) => !pack.valid).length);

/** 表头优先显示包名而不是文件名；包不在磁盘上时退回配置里记的原值 */
const selectedDisplayName = computed(() => selectedPack.value?.displayName ?? selectedName.value);

function backendHint(): string {
  switch (backend.value) {
    case 'iris':
      return t('shaderPacks.hintIris');
    case 'oculus':
      return t('shaderPacks.hintOculus');
    case 'optifine':
      return t('shaderPacks.hintOptiFine');
    default:
      return t('shaderPacks.hintUnknown');
  }
}

async function loadPacks() {
  // 快速切换实例时旧实例的慢响应后到会覆盖新列表，之后按旧文件名操作就会写错目录
  const target = props.instanceName;
  loading.value = true;
  error.value = '';
  try {
    const result = await shaderPackApi.getShaderPacks(target);
    if (props.instanceName !== target) return;
    state.value = result;
  } catch (e) {
    if (props.instanceName !== target) return;
    logError('Failed to load shader packs', e, 'ShaderPacksPanel');
    error.value = getErrorMessage(e);
  } finally {
    if (props.instanceName === target) {
      loading.value = false;
    }
  }
}

async function loadIsolationSetting() {
  try {
    const value = await configApi.loadConfigKey('versionIsolation');
    if (value !== null) versionIsolation.value = value === 'true';
  } catch (e) {
    logError('Failed to load isolation setting', e, 'ShaderPacksPanel');
  }
}

async function selectPack(pack: ShaderPackInfo) {
  if (busyFile.value || pack.selected) return;

  busyFile.value = pack.fileName;
  try {
    state.value = await shaderPackApi.selectShaderPack(props.instanceName, pack.fileName);
    emit('changed');
    notificationStore.success(t('shaderPacks.enableSuccess'), pack.displayName);
  } catch (e) {
    logError('Failed to select shader pack', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.enableFailed'), getErrorMessage(e), true);
  } finally {
    busyFile.value = null;
  }
}

async function toggleShaders() {
  if (toggling.value || !state.value) return;

  toggling.value = true;
  try {
    state.value = await shaderPackApi.setShadersEnabled(props.instanceName, !shadersEnabled.value);
    emit('changed');
  } catch (e) {
    logError('Failed to toggle shaders', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.toggleFailed'), getErrorMessage(e), true);
  } finally {
    toggling.value = false;
  }
}

async function disableShaders() {
  if (busyFile.value) return;

  busyFile.value = selectedName.value ?? '__off__';
  try {
    state.value = await shaderPackApi.disableShaderPack(props.instanceName);
    emit('changed');
    notificationStore.success(t('shaderPacks.disableSuccess'), t('shaderPacks.disableSuccessHint'));
  } catch (e) {
    logError('Failed to disable shaders', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.disableFailed'), getErrorMessage(e), true);
  } finally {
    busyFile.value = null;
  }
}

function askDeletePack(pack: ShaderPackInfo) {
  packToDelete.value = pack;
  deleteDialog.value = true;
}

async function deletePack() {
  if (!packToDelete.value) return;

  const fileName = packToDelete.value.fileName;
  busyFile.value = fileName;
  try {
    await shaderPackApi.deleteShaderPack(props.instanceName, fileName);
    deleteDialog.value = false;
    packToDelete.value = null;
    updates.value = updates.value.filter((item) => item.fileName !== fileName);
    await loadPacks();
    emit('changed');
  } catch (e) {
    logError('Failed to delete shader pack', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.deleteFailed'), getErrorMessage(e));
  } finally {
    busyFile.value = null;
  }
}

async function importPacks() {
  if (importing.value) return;

  try {
    const selected = await open({
      multiple: true,
      title: t('shaderPacks.pickTitle'),
      filters: [{ name: t('shaderPacks.pickFilter'), extensions: ['zip'] }],
    });
    if (!selected) return;

    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;

    importing.value = true;
    const summary = await shaderPackApi.importShaderPacks(props.instanceName, paths);
    await loadPacks();
    emit('changed');

    if (summary.imported.length === 0) {
      notificationStore.warning(
        t('shaderPacks.nothingImported'),
        summary.skipped.length > 0
          ? t('shaderPacks.nothingImportedSkipped', { names: summary.skipped.join(', ') })
          : t('shaderPacks.nothingImportedInvalid'),
      );
    } else if (summary.skipped.length > 0) {
      notificationStore.warning(
        t('shaderPacks.importedSome', { count: summary.imported.length }, summary.imported.length),
        t('shaderPacks.importedSkipped', { names: summary.skipped.join(', ') }),
      );
    } else {
      notificationStore.success(
        t('shaderPacks.importDone'),
        t('shaderPacks.importDoneHint', { count: summary.imported.length }, summary.imported.length),
      );
    }
  } catch (e) {
    logError('Failed to import shader packs', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.importFailed'), getErrorMessage(e), true);
  } finally {
    importing.value = false;
  }
}

async function openFolder() {
  try {
    await shaderPackApi.openFolder(props.instanceName);
  } catch (e) {
    logError('Failed to open shaderpacks folder', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.openFolderFailed'), getErrorMessage(e));
  }
}

function openBrowser() {
  browser.value = true;
}

async function onInstalled() {
  await loadPacks();
  emit('changed');
}

async function checkUpdates() {
  if (checking.value) return;

  checking.value = true;
  try {
    updates.value = await shaderPackApi.checkUpdates(props.instanceName);
    if (updates.value.length === 0) {
      notificationStore.success(t('shaderPacks.upToDate'), t('shaderPacks.upToDateHint'));
    } else {
      notificationStore.info(
        t('shaderPacks.updateFound'),
        t('shaderPacks.updateFoundHint', { count: updates.value.length }, updates.value.length),
      );
    }
  } catch (e) {
    logError('Failed to check shader pack updates', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.checkFailed'), getErrorMessage(e), true);
  } finally {
    checking.value = false;
  }
}

async function updatePack(pack: ShaderPackInfo) {
  const update = updateMap.value.get(pack.fileName);
  if (!update || updatingFile.value) return;

  updatingFile.value = pack.fileName;
  try {
    const installed = await shaderPackApi.updateShaderPack(props.instanceName, pack.fileName, update.versionId);
    updates.value = updates.value.filter((item) => item.fileName !== pack.fileName);
    await loadPacks();
    emit('changed');
    notificationStore.success(
      t('shaderPacks.updateSuccess'),
      t('shaderPacks.updateSuccessHint', { name: pack.displayName, version: installed.versionNumber }),
    );
  } catch (e) {
    logError('Failed to update shader pack', e, 'ShaderPacksPanel');
    notificationStore.error(t('shaderPacks.updateFailed'), getErrorMessage(e), true);
  } finally {
    updatingFile.value = null;
  }
}

async function updateAll() {
  if (updatingAll.value) return;

  const pending = [...updates.value];
  if (pending.length === 0) return;

  updatingAll.value = true;
  let succeeded = 0;
  let failed = 0;
  try {
    for (const update of pending) {
      try {
        await shaderPackApi.updateShaderPack(props.instanceName, update.fileName, update.versionId);
        succeeded += 1;
        updates.value = updates.value.filter((item) => item.fileName !== update.fileName);
      } catch (e) {
        failed += 1;
        logError(`Failed to update ${update.fileName}`, e, 'ShaderPacksPanel');
      }
    }

    await loadPacks();
    emit('changed');

    if (failed === 0) {
      notificationStore.success(
        t('shaderPacks.updateDone'),
        t('shaderPacks.updateDoneHint', { count: succeeded }, succeeded),
      );
    } else {
      notificationStore.warning(
        t('shaderPacks.updatePartlyFailed'),
        t('shaderPacks.updatePartlyFailedHint', { succeeded, failed }),
      );
    }
  } finally {
    updatingAll.value = false;
  }
}

onMounted(() => {
  void loadPacks();
  void loadIsolationSetting();
});

// 在实例之间切换时组件会被复用，必须按新实例重新拉列表
watch(
  () => props.instanceName,
  () => {
    updates.value = [];
    void loadPacks();
  },
);
</script>

<template>
  <div>
    <div class="d-flex align-center ga-2 mb-3 flex-wrap">
      <v-btn color="primary" variant="tonal" size="small" prepend-icon="mdi-plus" @click="openBrowser">
        {{ t('shaderPacks.install') }}
      </v-btn>
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-folder-open-outline"
        :loading="importing"
        @click="importPacks"
      >
        {{ t('shaderPacks.importFromFile') }}
      </v-btn>
      <v-btn variant="text" size="small" prepend-icon="mdi-update" :loading="checking" @click="checkUpdates">
        {{ t('shaderPacks.checkUpdates') }}
      </v-btn>
      <v-btn
        icon
        size="small"
        variant="text"
        :aria-label="t('shaderPacks.refreshList')"
        :loading="loading"
        @click="loadPacks"
      >
        <v-icon size="18">mdi-refresh</v-icon>
      </v-btn>

      <v-spacer />

      <v-btn
        v-if="updatableCount > 0"
        color="primary"
        variant="tonal"
        size="small"
        :loading="updatingAll"
        @click="updateAll"
      >
        {{ t('shaderPacks.updateAll', { count: updatableCount }) }}
      </v-btn>
    </div>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <v-card-text class="pa-4">
        <div class="d-flex align-center flex-wrap ga-3">
          <v-avatar size="40" rounded="lg" color="surface-container-high">
            <v-icon size="20" color="on-surface-variant">mdi-shimmer</v-icon>
          </v-avatar>

          <div class="flex-grow-1 min-width-0">
            <div class="text-title-small">
              <template v-if="shadersEnabled && selectedDisplayName">
                {{ t('shaderPacks.currentShader', { name: selectedDisplayName }) }}
              </template>
              <template v-else-if="shadersEnabled">{{ t('shaderPacks.enabledNoSelection') }}</template>
              <template v-else>{{ t('shaderPacks.disabled') }}</template>
            </div>
            <div class="text-body-small text-on-surface-variant">
              {{ t('shaderPacks.backendLine', { backend: backendLabel, hint: backendHint() }) }}
            </div>
          </div>

          <div class="d-flex align-center ga-1">
            <v-switch
              v-if="supportsToggle"
              :model-value="shadersEnabled"
              :loading="toggling"
              :disabled="toggling || packs.length === 0"
              :aria-label="t('shaderPacks.toggleAria')"
              :label="t('shaderPacks.toggleLabel')"
              hide-details
              inset
              density="compact"
              color="primary"
              @update:model-value="toggleShaders"
            />
            <v-btn
              v-else-if="shadersEnabled"
              variant="text"
              size="small"
              :loading="busyFile !== null"
              @click="disableShaders"
            >
              {{ t('shaderPacks.turnOff') }}
            </v-btn>
          </div>
        </div>

        <v-alert v-if="missingSelection" type="warning" variant="tonal" density="compact" class="mt-3">
          {{ t('shaderPacks.missingSelection') }} <strong>{{ missingSelection }}</strong>
          {{ t('shaderPacks.missingSelectionSuffix') }}
        </v-alert>
        <v-alert v-else-if="backend === 'unknown'" type="info" variant="tonal" density="compact" class="mt-3">
          {{ t('shaderPacks.noBackend') }}
        </v-alert>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container">
      <div v-if="loading && packs.length === 0" class="d-flex align-center justify-center pa-10">
        <v-progress-circular indeterminate size="28" color="primary" />
      </div>

      <div v-else-if="error" class="text-center pa-8">
        <div class="text-body-medium text-error mb-3">{{ error }}</div>
        <v-btn size="small" variant="tonal" color="primary" @click="loadPacks">{{ t('common.retry') }}</v-btn>
      </div>

      <div v-else-if="packs.length === 0" class="text-center pa-10 text-on-surface-variant">
        <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-shimmer</v-icon>
        <div class="text-body-medium">{{ t('shaderPacks.emptyTitle') }}</div>
        <div class="text-body-small mt-1">{{ t('shaderPacks.emptyHint') }}</div>
      </div>

      <v-list v-else lines="two" density="compact" bg-color="transparent">
        <v-list-item v-for="pack in packs" :key="pack.fileName">
          <template #prepend>
            <v-avatar size="40" rounded="lg" class="mr-3" color="surface-container-high">
              <v-icon size="20" :color="pack.selected && shadersEnabled ? 'primary' : 'on-surface-variant'">
                {{ pack.selected && shadersEnabled ? 'mdi-shimmer' : 'mdi-weather-sunset' }}
              </v-icon>
            </v-avatar>
          </template>

          <v-list-item-title
            class="text-body-medium"
            :class="{ 'text-on-surface-variant': !(pack.selected && shadersEnabled) }"
          >
            {{ pack.displayName }}
            <v-chip v-if="pack.selected && shadersEnabled" size="x-small" color="primary" variant="tonal" class="ml-2">
              {{ t('shaderPacks.inUse') }}
            </v-chip>
          </v-list-item-title>
          <v-list-item-subtitle class="text-body-small">
            {{ formatFileSize(pack.size) }} · {{ pack.fileName }}
            <template v-if="pack.isDirectory"> · {{ t('shaderPacks.folderSuffix') }}</template>
          </v-list-item-subtitle>

          <template #append>
            <div class="d-flex align-center ga-1">
              <v-chip v-if="!pack.valid" size="x-small" color="error" variant="tonal">
                {{ t('shaderPacks.missingShaders') }}
              </v-chip>
              <v-chip v-if="updateMap.get(pack.fileName)" size="x-small" color="warning" variant="tonal">
                {{ updateMap.get(pack.fileName)?.versionNumber }}
              </v-chip>
              <v-btn
                v-if="updateMap.get(pack.fileName)"
                icon
                size="small"
                variant="text"
                color="primary"
                :loading="updatingFile === pack.fileName"
                :disabled="updatingAll"
                :aria-label="t('shaderPacks.updateAria', { name: pack.displayName })"
                @click="updatePack(pack)"
              >
                <v-icon size="18">mdi-update</v-icon>
              </v-btn>
              <v-btn
                size="small"
                variant="tonal"
                color="primary"
                :loading="busyFile === pack.fileName"
                :disabled="pack.selected || busyFile !== null || backend === 'unknown'"
                :aria-label="t('shaderPacks.enableAria', { name: pack.displayName })"
                @click="selectPack(pack)"
              >
                {{ pack.selected ? t('shaderPacks.selected') : t('shaderPacks.enable') }}
              </v-btn>
              <v-btn
                icon
                size="small"
                variant="text"
                :aria-label="t('shaderPacks.deleteAria', { name: pack.displayName })"
                :disabled="busyFile === pack.fileName"
                @click="askDeletePack(pack)"
              >
                <v-icon size="18">mdi-delete-outline</v-icon>
              </v-btn>
            </div>
          </template>
        </v-list-item>
      </v-list>

      <v-divider v-if="packs.length > 0" />
      <div
        v-if="packs.length > 0"
        class="d-flex align-center flex-wrap ga-2 text-body-small text-on-surface-variant pa-3"
      >
        <span>{{ t('shaderPacks.totalPacks', { count: packs.length }, packs.length) }}</span>
        <template v-if="invalidCount > 0">
          <span>·</span>
          <span class="text-error">{{ t('shaderPacks.invalidPacks', { count: invalidCount }, invalidCount) }}</span>
        </template>
        <v-spacer />
        <v-btn size="x-small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openFolder">
          {{ t('common.openFolder') }}
        </v-btn>
      </div>

      <v-divider v-if="!versionIsolation" />
      <div v-if="!versionIsolation" class="text-body-small text-error pa-3">
        {{ t('shaderPacks.sharedFolderWarning') }}
      </div>
    </v-card>

    <ShaderPackBrowserDialog
      v-model="browser"
      :instance-name="instanceName"
      :game-version="gameVersion"
      @installed="onInstalled"
    />

    <v-dialog v-model="deleteDialog" max-width="380">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-title-large font-weight-bold mb-2">{{ t('shaderPacks.deleteTitle') }}</div>
          <div class="text-body-medium text-on-surface-variant">
            {{ t('shaderPacks.deleteConfirmPrefix') }}
            <strong class="text-on-surface">{{ packToDelete?.displayName }}</strong>
            {{ t('shaderPacks.deleteConfirmMiddle') }} {{ packToDelete?.fileName }}
            {{ t('shaderPacks.deleteConfirmSuffix') }}
          </div>
          <div v-if="packToDelete?.selected" class="text-body-small text-error mt-3">
            {{ t('shaderPacks.deleteSelectedWarning') }}
          </div>
          <div v-if="!versionIsolation" class="text-body-small text-error mt-3">
            {{ t('shaderPacks.deleteSharedWarning') }}
          </div>
        </v-card-text>
        <v-card-actions class="pa-4 pt-0">
          <v-spacer />
          <v-btn variant="text" @click="deleteDialog = false">{{ t('common.cancel') }}</v-btn>
          <v-btn color="error" :loading="busyFile === packToDelete?.fileName" @click="deletePack">
            {{ t('common.delete') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
.min-width-0 {
  min-width: 0;
}
</style>
