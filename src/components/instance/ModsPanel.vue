<script setup lang="ts">
/**
 * 实例详情页的模组面板
 * 列表、启停、删除、安装（含依赖）、检查与批量更新都在这里，
 * 所有操作完成后通过 changed 通知父级刷新实例信息（模组数量会变）
 */
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import ModBrowserDialog from './ModBrowserDialog.vue';
import { useNotificationStore } from '../../stores/notificationStore';
import { configApi, instanceApi, modApi } from '../../services';
import type { ModInfo, ModUpdate } from '../../types/events';
import { getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const props = defineProps<{
  instanceName: string;
  gameVersion?: string | null;
  loader?: string | null;
}>();

const emit = defineEmits<{
  (e: 'changed'): void;
}>();

const notificationStore = useNotificationStore();
const { t } = useI18n();

const mods = ref<ModInfo[]>([]);
const loading = ref(false);
const error = ref('');
const busyFile = ref<string | null>(null);
const deleteDialog = ref(false);
const modToDelete = ref<ModInfo | null>(null);
const browser = ref(false);
const updates = ref<ModUpdate[]>([]);
const checking = ref(false);
const updatingFile = ref<string | null>(null);
const updatingAll = ref(false);
// 关闭版本隔离时模组在共享目录里，启停/删除会影响所有实例
const versionIsolation = ref(true);

const updateMap = computed(() => {
  const map = new Map<string, ModUpdate>();
  for (const update of updates.value) {
    map.set(update.fileName, update);
  }
  return map;
});

const updatableCount = computed(() => updates.value.length);

async function loadMods() {
  // 快速切换实例时旧实例的慢响应后到会覆盖新列表，之后按旧文件名操作就会写错目录
  const target = props.instanceName;
  loading.value = true;
  error.value = '';
  try {
    const list = await instanceApi.getMods(target);
    if (props.instanceName !== target) return;
    mods.value = list;
  } catch (e) {
    if (props.instanceName !== target) return;
    logError('Failed to load mods', e, 'ModsPanel');
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
    logError('Failed to load isolation setting', e, 'ModsPanel');
  }
}

async function toggleMod(mod: ModInfo) {
  busyFile.value = mod.fileName;
  try {
    await instanceApi.setModEnabled(props.instanceName, mod.fileName, !mod.enabled);
    await loadMods();
    emit('changed');
  } catch (e) {
    logError('Failed to toggle mod', e, 'ModsPanel');
    notificationStore.error(t('mods.toggleFailed'), getErrorMessage(e));
  } finally {
    busyFile.value = null;
  }
}

function askDeleteMod(mod: ModInfo) {
  modToDelete.value = mod;
  deleteDialog.value = true;
}

async function deleteMod() {
  if (!modToDelete.value) return;

  const fileName = modToDelete.value.fileName;
  busyFile.value = fileName;
  try {
    await instanceApi.deleteMod(props.instanceName, fileName);
    deleteDialog.value = false;
    modToDelete.value = null;
    await loadMods();
    emit('changed');
  } catch (e) {
    logError('Failed to delete mod', e, 'ModsPanel');
    notificationStore.error(t('mods.deleteFailed'), getErrorMessage(e));
  } finally {
    busyFile.value = null;
  }
}

function openBrowser() {
  browser.value = true;
}

async function onInstalled() {
  await loadMods();
  emit('changed');
}

async function checkUpdates() {
  if (checking.value) return;

  checking.value = true;
  try {
    updates.value = await modApi.checkModUpdates(props.instanceName);
    if (updates.value.length === 0) {
      notificationStore.success(t('mods.upToDate'), t('mods.upToDateHint'));
    } else {
      notificationStore.info(
        t('mods.updateFound'),
        t('mods.updateFoundHint', { count: updates.value.length }, updates.value.length),
      );
    }
  } catch (e) {
    logError('Failed to check mod updates', e, 'ModsPanel');
    notificationStore.error(t('mods.checkFailed'), getErrorMessage(e), true);
  } finally {
    checking.value = false;
  }
}

async function updateMod(mod: ModInfo) {
  const update = updateMap.value.get(mod.fileName);
  if (!update || updatingFile.value) return;

  updatingFile.value = mod.fileName;
  try {
    await modApi.updateMod(props.instanceName, mod.fileName, update.versionId);
    updates.value = updates.value.filter((item) => item.fileName !== mod.fileName);
    await loadMods();
    emit('changed');
    notificationStore.success(
      t('mods.updateSuccess'),
      t('mods.updateSuccessHint', { name: mod.displayName, version: update.versionNumber }),
    );
  } catch (e) {
    logError('Failed to update mod', e, 'ModsPanel');
    notificationStore.error(t('mods.updateFailed'), getErrorMessage(e), true);
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
        await modApi.updateMod(props.instanceName, update.fileName, update.versionId);
        succeeded += 1;
        updates.value = updates.value.filter((item) => item.fileName !== update.fileName);
      } catch (e) {
        failed += 1;
        logError(`Failed to update ${update.fileName}`, e, 'ModsPanel');
      }
    }

    await loadMods();
    emit('changed');

    if (failed === 0) {
      notificationStore.success(t('mods.updateDone'), t('mods.updateDoneHint', { count: succeeded }, succeeded));
    } else {
      notificationStore.warning(t('mods.updatePartlyFailed'), t('mods.updatePartlyFailedHint', { succeeded, failed }));
    }
  } finally {
    updatingAll.value = false;
  }
}

function formatModSize(bytes: number): string {
  return bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

onMounted(() => {
  void loadMods();
  void loadIsolationSetting();
});

// 在实例之间切换时组件会被复用，必须按新实例重新拉列表
watch(
  () => props.instanceName,
  () => {
    updates.value = [];
    void loadMods();
  },
);
</script>

<template>
  <div>
    <div class="d-flex align-center ga-2 mb-3 flex-wrap">
      <v-btn color="primary" variant="tonal" size="small" prepend-icon="mdi-plus" @click="openBrowser">
        {{ t('mods.install') }}
      </v-btn>
      <v-btn variant="text" size="small" prepend-icon="mdi-update" :loading="checking" @click="checkUpdates">
        {{ t('mods.checkUpdates') }}
      </v-btn>
      <v-btn icon size="small" variant="text" :aria-label="t('mods.refreshList')" :loading="loading" @click="loadMods">
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
        {{ t('mods.updateAll', { count: updatableCount }) }}
      </v-btn>
    </div>

    <v-card variant="flat" color="surface-container">
      <div v-if="loading && mods.length === 0" class="d-flex align-center justify-center pa-10">
        <v-progress-circular indeterminate size="28" color="primary" />
      </div>

      <div v-else-if="error" class="text-center pa-8">
        <div class="text-body-medium text-error mb-3">{{ error }}</div>
        <v-btn size="small" variant="tonal" color="primary" @click="loadMods">{{ t('common.retry') }}</v-btn>
      </div>

      <div v-else-if="mods.length === 0" class="text-center pa-10 text-on-surface-variant">
        <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-puzzle-outline</v-icon>
        <div class="text-body-medium">{{ t('mods.emptyTitle') }}</div>
        <div class="text-body-small mt-1">{{ t('mods.emptyHint') }}</div>
      </div>

      <v-list v-else lines="two" density="compact">
        <v-list-item v-for="mod in mods" :key="mod.fileName">
          <template #prepend>
            <v-icon size="20" color="on-surface-variant">mdi-puzzle-outline</v-icon>
          </template>
          <v-list-item-title class="text-body-medium" :class="{ 'text-on-surface-variant': !mod.enabled }">
            {{ mod.displayName }}
          </v-list-item-title>
          <v-list-item-subtitle class="text-body-small">
            <template v-if="mod.version">v{{ mod.version }} · </template>
            {{ formatModSize(mod.size) }} · {{ mod.fileName }}
          </v-list-item-subtitle>

          <template #append>
            <div class="d-flex align-center ga-1">
              <v-chip v-if="updateMap.get(mod.fileName)" size="x-small" color="warning" variant="tonal">
                {{ updateMap.get(mod.fileName)?.versionNumber }}
              </v-chip>
              <v-btn
                v-if="updateMap.get(mod.fileName)"
                icon
                size="small"
                variant="text"
                color="primary"
                :loading="updatingFile === mod.fileName"
                :disabled="updatingAll"
                :aria-label="t('mods.updateAria', { name: mod.displayName })"
                @click="updateMod(mod)"
              >
                <v-icon size="18">mdi-update</v-icon>
              </v-btn>
              <v-switch
                :model-value="mod.enabled"
                :loading="busyFile === mod.fileName"
                :disabled="busyFile === mod.fileName"
                :aria-label="
                  mod.enabled
                    ? t('mods.disableAria', { name: mod.displayName })
                    : t('mods.enableAria', { name: mod.displayName })
                "
                hide-details
                inset
                density="compact"
                color="primary"
                @update:model-value="toggleMod(mod)"
              />
              <v-btn
                icon
                size="small"
                variant="text"
                :aria-label="t('mods.deleteAria', { name: mod.displayName })"
                :disabled="busyFile === mod.fileName"
                @click="askDeleteMod(mod)"
              >
                <v-icon size="18">mdi-delete-outline</v-icon>
              </v-btn>
            </div>
          </template>
        </v-list-item>
      </v-list>

      <v-divider v-if="mods.length > 0" />
      <div v-if="mods.length > 0" class="text-body-small text-on-surface-variant pa-3">
        {{ versionIsolation ? t('mods.disableHint') : t('mods.sharedFolderHint') }}
      </div>
    </v-card>

    <ModBrowserDialog
      v-model="browser"
      :instance-name="instanceName"
      :game-version="gameVersion"
      :loader="loader"
      @installed="onInstalled"
    />

    <v-dialog v-model="deleteDialog" max-width="380">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-title-large font-weight-bold mb-2">{{ t('mods.deleteTitle') }}</div>
          <div class="text-body-medium text-on-surface-variant">
            {{ t('mods.deleteConfirmPrefix') }}
            <strong class="text-on-surface">{{ modToDelete?.displayName }}</strong>
            {{ t('mods.deleteConfirmMiddle') }} {{ modToDelete?.fileName }} {{ t('mods.deleteConfirmSuffix') }}
          </div>
          <div v-if="!versionIsolation" class="text-body-small text-error mt-3">
            {{ t('mods.deleteSharedWarning') }}
          </div>
        </v-card-text>
        <v-card-actions class="pa-4 pt-0">
          <v-spacer />
          <v-btn variant="text" @click="deleteDialog = false">{{ t('common.cancel') }}</v-btn>
          <v-btn color="error" :loading="busyFile === modToDelete?.fileName" @click="deleteMod">
            {{ t('common.delete') }}
          </v-btn>
        </v-card-actions>
      </v-card>
    </v-dialog>
  </div>
</template>
