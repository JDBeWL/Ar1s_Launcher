<script setup lang="ts">
/**
 * 实例详情页的资源包面板
 * 启用状态由游戏自己记在 options.txt 的 resourcePacks 列表里，所以这里的开关改的是配置而不是文件名。
 */
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { open } from '@tauri-apps/plugin-dialog';
import ResourcePackBrowserDialog from './ResourcePackBrowserDialog.vue';
import { useNotificationStore } from '../../stores/notificationStore';
import { configApi, resourcePackApi } from '../../services';
import type { ResourcePackInfo, ResourcePackUpdate } from '../../types/events';
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

const packs = ref<ResourcePackInfo[]>([]);
const loading = ref(false);
const error = ref('');
const busyFile = ref<string | null>(null);
const importing = ref(false);
const deleteDialog = ref(false);
const packToDelete = ref<ResourcePackInfo | null>(null);
const browser = ref(false);
const updates = ref<ResourcePackUpdate[]>([]);
const checking = ref(false);
const updatingFile = ref<string | null>(null);
const updatingAll = ref(false);
// 关闭版本隔离时资源包放在共享目录，启停会影响所有实例
const versionIsolation = ref(true);

const updateMap = computed(() => {
  const map = new Map<string, ResourcePackUpdate>();
  for (const update of updates.value) {
    map.set(update.fileName, update);
  }
  return map;
});

const updatableCount = computed(() => updates.value.length);

const enabledCount = computed(() => packs.value.filter((pack) => pack.enabled).length);

/** pack.mcmeta 里声明的格式区间，读不到就不显示 */
function formatRange(pack: ResourcePackInfo): string {
  if (pack.minFormat === null && pack.maxFormat === null) return '';
  if (pack.minFormat === null) return t('resourcePacks.formatSingle', { format: pack.maxFormat });
  if (pack.maxFormat === null || pack.minFormat === pack.maxFormat) {
    return t('resourcePacks.formatSingle', { format: pack.minFormat });
  }
  return t('resourcePacks.formatRange', { range: `${pack.minFormat}–${pack.maxFormat}` });
}

function compatibilityHint(pack: ResourcePackInfo): { text: string; color: string } | null {
  switch (pack.compatibility) {
    case 'outdated':
      return { text: t('resourcePacks.outdated'), color: 'warning' };
    case 'tooNew':
      return { text: t('resourcePacks.tooNew'), color: 'error' };
    default:
      return null;
  }
}

async function loadPacks() {
  // 快速切换实例时旧实例的慢响应后到会覆盖新列表，之后按旧文件名操作就会写错目录
  const target = props.instanceName;
  loading.value = true;
  error.value = '';
  try {
    const list = await resourcePackApi.getResourcePacks(target);
    if (props.instanceName !== target) return;
    packs.value = list;
  } catch (e) {
    if (props.instanceName !== target) return;
    logError('Failed to load resource packs', e, 'ResourcePacksPanel');
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
    logError('Failed to load isolation setting', e, 'ResourcePacksPanel');
  }
}

async function togglePack(pack: ResourcePackInfo) {
  busyFile.value = pack.fileName;
  try {
    await resourcePackApi.setResourcePackEnabled(props.instanceName, pack.fileName, !pack.enabled);
    await loadPacks();
    emit('changed');
  } catch (e) {
    logError('Failed to toggle resource pack', e, 'ResourcePacksPanel');
    notificationStore.error(t('resourcePacks.toggleFailed'), getErrorMessage(e));
  } finally {
    busyFile.value = null;
  }
}

function askDeletePack(pack: ResourcePackInfo) {
  packToDelete.value = pack;
  deleteDialog.value = true;
}

async function deletePack() {
  if (!packToDelete.value) return;

  const fileName = packToDelete.value.fileName;
  busyFile.value = fileName;
  try {
    await resourcePackApi.deleteResourcePack(props.instanceName, fileName);
    deleteDialog.value = false;
    packToDelete.value = null;
    updates.value = updates.value.filter((item) => item.fileName !== fileName);
    await loadPacks();
    emit('changed');
  } catch (e) {
    logError('Failed to delete resource pack', e, 'ResourcePacksPanel');
    notificationStore.error(t('resourcePacks.deleteFailed'), getErrorMessage(e));
  } finally {
    busyFile.value = null;
  }
}

async function importPacks() {
  if (importing.value) return;

  try {
    const selected = await open({
      multiple: true,
      title: t('resourcePacks.pickTitle'),
      filters: [{ name: t('resourcePacks.pickFilter'), extensions: ['zip'] }],
    });
    if (!selected) return;

    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;

    importing.value = true;
    const summary = await resourcePackApi.importResourcePacks(props.instanceName, paths);
    await loadPacks();
    emit('changed');

    if (summary.imported.length === 0) {
      notificationStore.warning(
        t('resourcePacks.nothingImported'),
        summary.skipped.length > 0
          ? t('resourcePacks.nothingImportedSkipped', { names: summary.skipped.join(', ') })
          : t('resourcePacks.nothingImportedInvalid'),
      );
    } else if (summary.skipped.length > 0) {
      notificationStore.warning(
        t('resourcePacks.importedSome', { count: summary.imported.length }, summary.imported.length),
        t('resourcePacks.importedSkipped', { names: summary.skipped.join(', ') }),
      );
    } else {
      notificationStore.success(
        t('resourcePacks.importDone'),
        t('resourcePacks.importDoneHint', { count: summary.imported.length }, summary.imported.length),
      );
    }
  } catch (e) {
    logError('Failed to import resource packs', e, 'ResourcePacksPanel');
    notificationStore.error(t('resourcePacks.importFailed'), getErrorMessage(e), true);
  } finally {
    importing.value = false;
  }
}

async function openFolder() {
  try {
    await resourcePackApi.openFolder(props.instanceName);
  } catch (e) {
    logError('Failed to open resourcepacks folder', e, 'ResourcePacksPanel');
    notificationStore.error(t('resourcePacks.openFolderFailed'), getErrorMessage(e));
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
    updates.value = await resourcePackApi.checkUpdates(props.instanceName);
    if (updates.value.length === 0) {
      notificationStore.success(t('resourcePacks.upToDate'), t('resourcePacks.upToDateHint'));
    } else {
      notificationStore.info(
        t('resourcePacks.updateFound'),
        t('resourcePacks.updateFoundHint', { count: updates.value.length }, updates.value.length),
      );
    }
  } catch (e) {
    logError('Failed to check resource pack updates', e, 'ResourcePacksPanel');
    notificationStore.error(t('resourcePacks.checkFailed'), getErrorMessage(e), true);
  } finally {
    checking.value = false;
  }
}

async function updatePack(pack: ResourcePackInfo) {
  const update = updateMap.value.get(pack.fileName);
  if (!update || updatingFile.value) return;

  updatingFile.value = pack.fileName;
  try {
    const installed = await resourcePackApi.updateResourcePack(props.instanceName, pack.fileName, update.versionId);
    updates.value = updates.value.filter((item) => item.fileName !== pack.fileName);
    await loadPacks();
    emit('changed');
    notificationStore.success(
      t('resourcePacks.updateSuccess'),
      t('resourcePacks.updateSuccessHint', { name: pack.displayName, version: installed.versionNumber }),
    );
  } catch (e) {
    logError('Failed to update resource pack', e, 'ResourcePacksPanel');
    notificationStore.error(t('resourcePacks.updateFailed'), getErrorMessage(e), true);
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
        await resourcePackApi.updateResourcePack(props.instanceName, update.fileName, update.versionId);
        succeeded += 1;
        updates.value = updates.value.filter((item) => item.fileName !== update.fileName);
      } catch (e) {
        failed += 1;
        logError(`Failed to update ${update.fileName}`, e, 'ResourcePacksPanel');
      }
    }

    await loadPacks();
    emit('changed');

    if (failed === 0) {
      notificationStore.success(
        t('resourcePacks.updateDone'),
        t('resourcePacks.updateDoneHint', { count: succeeded }, succeeded),
      );
    } else {
      notificationStore.warning(
        t('resourcePacks.updatePartlyFailed'),
        t('resourcePacks.updatePartlyFailedHint', { succeeded, failed }),
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
        {{ t('resourcePacks.install') }}
      </v-btn>
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-folder-open-outline"
        :loading="importing"
        @click="importPacks"
      >
        {{ t('resourcePacks.importFromFile') }}
      </v-btn>
      <v-btn variant="text" size="small" prepend-icon="mdi-update" :loading="checking" @click="checkUpdates">
        {{ t('resourcePacks.checkUpdates') }}
      </v-btn>
      <v-btn
        icon
        size="small"
        variant="text"
        :aria-label="t('resourcePacks.refreshList')"
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
        {{ t('resourcePacks.updateAll', { count: updatableCount }) }}
      </v-btn>
    </div>

    <v-card variant="flat" color="surface-container">
      <div v-if="loading && packs.length === 0" class="d-flex align-center justify-center pa-10">
        <v-progress-circular indeterminate size="28" color="primary" />
      </div>

      <div v-else-if="error" class="text-center pa-8">
        <div class="text-body-medium text-error mb-3">{{ error }}</div>
        <v-btn size="small" variant="tonal" color="primary" @click="loadPacks">{{ t('common.retry') }}</v-btn>
      </div>

      <div v-else-if="packs.length === 0" class="text-center pa-10 text-on-surface-variant">
        <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-texture-box</v-icon>
        <div class="text-body-medium">{{ t('resourcePacks.emptyTitle') }}</div>
        <div class="text-body-small mt-1">{{ t('resourcePacks.emptyHint') }}</div>
      </div>

      <v-list v-else lines="two" density="compact">
        <v-list-item v-for="pack in packs" :key="pack.fileName">
          <template #prepend>
            <v-avatar size="40" rounded="lg" class="mr-3" color="surface-container-high">
              <v-img v-if="pack.iconDataUrl" :src="pack.iconDataUrl" cover>
                <template #error>
                  <v-icon size="20" color="on-surface-variant">mdi-texture-box</v-icon>
                </template>
              </v-img>
              <v-icon v-else size="20" color="on-surface-variant">mdi-texture-box</v-icon>
            </v-avatar>
          </template>

          <v-list-item-title class="text-body-medium" :class="{ 'text-on-surface-variant': !pack.enabled }">
            {{ pack.displayName }}
          </v-list-item-title>
          <v-list-item-subtitle class="text-body-small">
            <template v-if="formatRange(pack)">{{ formatRange(pack) }} · </template>
            {{ formatFileSize(pack.size) }} · {{ pack.fileName }}
            <template v-if="pack.isDirectory"> · {{ t('resourcePacks.folderSuffix') }}</template>
          </v-list-item-subtitle>

          <template #append>
            <div class="d-flex align-center ga-1">
              <v-chip
                v-if="compatibilityHint(pack)"
                size="x-small"
                :color="compatibilityHint(pack)?.color"
                variant="tonal"
              >
                {{ compatibilityHint(pack)?.text }}
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
                :aria-label="t('resourcePacks.updateAria', { name: pack.displayName })"
                @click="updatePack(pack)"
              >
                <v-icon size="18">mdi-update</v-icon>
              </v-btn>
              <v-switch
                :model-value="pack.enabled"
                :loading="busyFile === pack.fileName"
                :disabled="busyFile === pack.fileName"
                :aria-label="
                  pack.enabled
                    ? t('resourcePacks.disableAria', { name: pack.displayName })
                    : t('resourcePacks.enableAria', { name: pack.displayName })
                "
                hide-details
                inset
                density="compact"
                color="primary"
                @update:model-value="togglePack(pack)"
              />
              <v-btn
                icon
                size="small"
                variant="text"
                :aria-label="t('resourcePacks.deleteAria', { name: pack.displayName })"
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
        <span>{{ t('resourcePacks.enabledCount', { enabled: enabledCount, total: packs.length }) }}</span>
        <span>·</span>
        <span>{{ t('resourcePacks.enabledHint') }}</span>
        <v-spacer />
        <v-btn size="x-small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openFolder">
          {{ t('common.openFolder') }}
        </v-btn>
      </div>

      <v-divider v-if="!versionIsolation" />
      <div v-if="!versionIsolation" class="text-body-small text-error pa-3">
        {{ t('resourcePacks.sharedFolderWarning') }}
      </div>
    </v-card>

    <ResourcePackBrowserDialog
      v-model="browser"
      :instance-name="instanceName"
      :game-version="gameVersion"
      @installed="onInstalled"
    />

    <v-dialog v-model="deleteDialog" max-width="380">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-title-large font-weight-bold mb-2">{{ t('resourcePacks.deleteTitle') }}</div>
          <div class="text-body-medium text-on-surface-variant">
            {{ t('resourcePacks.deleteConfirmPrefix') }}
            <strong class="text-on-surface">{{ packToDelete?.displayName }}</strong>
            {{ t('resourcePacks.deleteConfirmMiddle') }} {{ packToDelete?.fileName }}
            {{ t('resourcePacks.deleteConfirmSuffix') }}
          </div>
          <div v-if="!versionIsolation" class="text-body-small text-error mt-3">
            {{ t('resourcePacks.deleteSharedWarning') }}
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
