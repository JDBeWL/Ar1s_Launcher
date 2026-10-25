<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { useI18n } from 'vue-i18n';
import { useInstanceScope } from '../../composables/useInstanceScope';
import { useNotificationStore } from '../../stores/notificationStore';
import { configApi, saveApi } from '../../services';
import type { SaveBackupInfo, SaveInfo } from '../../types/events';
import { formatFileSize, formatTimeAgo, getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const props = defineProps<{ instanceName: string }>();
const { t } = useI18n();
const notificationStore = useNotificationStore();
const scope = useInstanceScope(() => props.instanceName);

const saves = ref<SaveInfo[]>([]);
const backups = ref<SaveBackupInfo[]>([]);
const loading = ref(true);
const error = ref('');
const busyKey = ref<string | null>(null);
const autoBackup = ref(false);
const retention = ref(3);
const versionIsolation = ref(true);

async function load() {
  const task = scope.begin();
  loading.value = true;
  error.value = '';
  try {
    const [saveList, backupList, autoValue, retentionValue, isolationValue] = await Promise.all([
      saveApi.getSaves(task.instanceName),
      saveApi.getBackups(task.instanceName),
      configApi.loadConfigKey('saveAutoBackup'),
      configApi.loadConfigKey('saveBackupRetention'),
      configApi.loadConfigKey('versionIsolation'),
    ]);
    if (!scope.isCurrent(task)) return;
    saves.value = saveList;
    backups.value = backupList;
    autoBackup.value = autoValue === 'true';
    retention.value = Number(retentionValue) || 3;
    versionIsolation.value = isolationValue !== 'false';
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load saves', e, 'SavesPanel');
    error.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) loading.value = false;
  }
}

async function toggleAutoBackup(value: boolean | null) {
  autoBackup.value = value === true;
  try {
    await configApi.saveConfigKey('saveAutoBackup', String(autoBackup.value));
    notificationStore.info(autoBackup.value ? t('saves.autoBackupOn') : t('saves.autoBackupOff'));
  } catch (e) {
    autoBackup.value = !autoBackup.value;
    logError('Failed to save auto backup setting', e, 'SavesPanel');
    notificationStore.error(t('saves.saveFailed'), getErrorMessage(e));
  }
}

async function commitRetention(event: Event) {
  const raw = Number((event.target as HTMLInputElement).value);
  retention.value = Math.min(Math.max(Math.round(raw) || 3, 1), 50);
  try {
    await configApi.saveConfigKey('saveBackupRetention', String(retention.value));
  } catch (e) {
    logError('Failed to save backup retention', e, 'SavesPanel');
    notificationStore.error(t('saves.saveFailed'), getErrorMessage(e));
  }
}

async function backupSave(save: SaveInfo) {
  const task = scope.begin();
  busyKey.value = `backup:${save.folder}`;
  try {
    await saveApi.backup(task.instanceName, save.folder);
    if (!scope.isCurrent(task)) return;
    notificationStore.success(t('saves.backupSuccess'), t('saves.backupSuccessHint', { name: save.displayName }));
    await load();
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to back up save', e, 'SavesPanel');
    notificationStore.error(t('saves.backupFailed'), getErrorMessage(e), true);
  } finally {
    if (scope.isCurrent(task)) busyKey.value = null;
  }
}

async function confirmRestore(backup: SaveBackupInfo) {
  const confirmed = await notificationStore.confirm(
    t('saves.restoreTitle'),
    t('saves.restoreConfirm', {
      name: backup.displayName,
      time: formatTimeAgo(backup.createdAt),
    }),
    'warning',
  );
  if (!confirmed) return;

  const task = scope.begin();
  busyKey.value = `restore:${backup.world}:${backup.fileName}`;
  try {
    await saveApi.restore(task.instanceName, backup.world, backup.fileName);
    if (!scope.isCurrent(task)) return;
    notificationStore.success(t('saves.restoreSuccess'), t('saves.restoreSuccessHint', { name: backup.displayName }));
    await load();
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to restore save', e, 'SavesPanel');
    notificationStore.error(t('saves.restoreFailed'), getErrorMessage(e), true);
  } finally {
    if (scope.isCurrent(task)) busyKey.value = null;
  }
}

async function confirmDelete(backup: SaveBackupInfo) {
  const confirmed = await notificationStore.confirm(
    t('saves.deleteBackupTitle'),
    t('saves.deleteBackupConfirm', {
      name: backup.displayName,
      time: formatTimeAgo(backup.createdAt),
    }),
    'warning',
  );
  if (!confirmed) return;

  const task = scope.begin();
  busyKey.value = `delete:${backup.world}:${backup.fileName}`;
  try {
    await saveApi.deleteBackup(task.instanceName, backup.world, backup.fileName);
    if (!scope.isCurrent(task)) return;
    notificationStore.success(t('saves.deleteSuccess'));
    await load();
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to delete save backup', e, 'SavesPanel');
    notificationStore.error(t('saves.deleteFailed'), getErrorMessage(e));
  } finally {
    if (scope.isCurrent(task)) busyKey.value = null;
  }
}

async function importFolder() {
  const selected = await open({
    directory: true,
    multiple: true,
    title: t('saves.importFolder'),
  });
  if (!selected) return;
  await importPaths(Array.isArray(selected) ? selected : [selected]);
}

async function importZip() {
  const selected = await open({
    multiple: true,
    title: t('saves.importZip'),
    filters: [{ name: 'ZIP', extensions: ['zip'] }],
  });
  if (!selected) return;
  await importPaths(Array.isArray(selected) ? selected : [selected]);
}

async function importPaths(paths: string[]) {
  if (paths.length === 0) return;
  const task = scope.begin();
  busyKey.value = 'import';
  try {
    const summary = await saveApi.importSaves(task.instanceName, paths);
    if (!scope.isCurrent(task)) return;
    notificationStore.info(
      summary.skipped.length > 0 ? t('saves.importPartial') : t('saves.importSuccess'),
      t('saves.importResult', {
        imported: summary.imported.length,
        skipped: summary.skipped.length,
      }),
    );
    await load();
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to import saves', e, 'SavesPanel');
    notificationStore.error(t('saves.importFailed'), getErrorMessage(e), true);
  } finally {
    if (scope.isCurrent(task)) busyKey.value = null;
  }
}

async function openSavesFolder() {
  try {
    await saveApi.openSavesFolder(props.instanceName);
  } catch (e) {
    logError('Failed to open saves folder', e, 'SavesPanel');
    notificationStore.error(t('saves.openFolderFailed'), getErrorMessage(e));
  }
}

function kindLabel(kind: SaveBackupInfo['kind']): string {
  if (kind === 'manual') return t('saves.kindManual');
  if (kind === 'auto') return t('saves.kindAuto');
  return t('saves.kindPreRestore');
}

function lastPlayedLabel(save: SaveInfo): string {
  if (!save.lastPlayed) return '—';
  return formatTimeAgo(save.lastPlayed);
}

onMounted(load);
watch(
  () => props.instanceName,
  () => void load(),
);
</script>

<template>
  <div class="saves-panel">
    <div class="d-flex align-center ga-2 mb-3 flex-wrap">
      <v-btn
        color="primary"
        variant="tonal"
        size="small"
        prepend-icon="mdi-folder-open-outline"
        :disabled="busyKey !== null"
        @click="importFolder"
      >
        {{ t('saves.importFolder') }}
      </v-btn>
      <v-btn
        variant="text"
        size="small"
        prepend-icon="mdi-zip-box-outline"
        :disabled="busyKey !== null"
        @click="importZip"
      >
        {{ t('saves.importZip') }}
      </v-btn>
      <v-spacer />
      <v-btn icon size="small" variant="text" :aria-label="t('saves.refresh')" :loading="loading" @click="load">
        <v-icon size="18">mdi-refresh</v-icon>
      </v-btn>
    </div>

    <v-alert v-if="!versionIsolation" type="warning" variant="tonal" density="comfortable" class="mb-3">
      {{ t('saves.isolationWarning') }}
    </v-alert>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <v-card-text class="pa-4">
        <div class="d-flex align-center justify-space-between mb-2">
          <div class="d-flex align-center">
            <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-content-save-cog-outline</v-icon>
            <div>
              <div class="text-title-small">{{ t('saves.autoBackupTitle') }}</div>
              <div class="text-body-small text-on-surface-variant">{{ t('saves.autoBackupHint') }}</div>
            </div>
          </div>
          <v-switch
            :model-value="autoBackup"
            :aria-label="t('saves.autoBackupTitle')"
            hide-details
            inset
            density="compact"
            color="primary"
            @update:model-value="toggleAutoBackup"
          />
        </div>
        <div class="d-flex align-center flex-wrap ga-3">
          <v-text-field
            :model-value="retention"
            type="number"
            density="compact"
            :label="t('saves.retentionLabel')"
            :suffix="t('saves.retentionSuffix')"
            min="1"
            max="50"
            hide-details
            hide-spin-buttons
            style="max-width: 220px"
            @change="commitRetention"
          />
          <span class="text-body-small text-on-surface-variant">{{ t('saves.retentionHint') }}</span>
        </div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" color="surface-container" class="mb-3">
      <div v-if="loading && saves.length === 0" class="d-flex align-center justify-center pa-10">
        <v-progress-circular indeterminate size="28" color="primary" />
      </div>

      <div v-else-if="error" class="text-center pa-8">
        <div class="text-body-medium text-error mb-3">{{ error }}</div>
        <v-btn size="small" variant="tonal" color="primary" @click="load">{{ t('common.retry') }}</v-btn>
      </div>

      <div v-else-if="saves.length === 0" class="text-center pa-10 text-on-surface-variant">
        <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-content-save-off-outline</v-icon>
        <div class="text-body-medium">{{ t('saves.emptyTitle') }}</div>
        <div class="text-body-small mt-1">{{ t('saves.emptyHint') }}</div>
      </div>

      <template v-else>
        <v-list density="compact" lines="two" bg-color="transparent">
          <v-list-item v-for="save in saves" :key="save.folder">
            <template #prepend>
              <v-avatar size="40" rounded="lg" color="surface-container-high" class="mr-3">
                <v-img v-if="save.iconDataUrl" :src="save.iconDataUrl" cover>
                  <template #placeholder>
                    <v-icon size="20" color="on-surface-variant">mdi-cube-outline</v-icon>
                  </template>
                </v-img>
                <v-icon v-else size="20" color="on-surface-variant">mdi-cube-outline</v-icon>
              </v-avatar>
            </template>
            <v-list-item-title class="text-body-medium">{{ save.displayName }}</v-list-item-title>
            <v-list-item-subtitle class="text-body-small">
              {{ save.folder }} · {{ t('saves.lastPlayed', { time: lastPlayedLabel(save) }) }} ·
              {{ formatFileSize(save.size) }}
            </v-list-item-subtitle>
            <template #append>
              <v-btn
                size="small"
                variant="tonal"
                color="primary"
                prepend-icon="mdi-content-save-plus-outline"
                :loading="busyKey === `backup:${save.folder}`"
                :disabled="busyKey !== null"
                @click="backupSave(save)"
              >
                {{ t('saves.backup') }}
              </v-btn>
            </template>
          </v-list-item>
        </v-list>
        <v-divider />
        <div class="d-flex align-center flex-wrap ga-2 text-body-small text-on-surface-variant pa-3">
          <span>{{ t('saves.saveCount', { count: saves.length }) }}</span>
          <v-spacer />
          <v-btn size="x-small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openSavesFolder">
            {{ t('saves.openFolder') }}
          </v-btn>
        </div>
      </template>
    </v-card>

    <v-card v-if="!error && (!loading || backups.length > 0)" variant="flat" color="surface-container">
      <v-list density="compact" lines="two" bg-color="transparent">
        <v-list-subheader class="text-title-medium">{{ t('saves.backupSection') }}</v-list-subheader>
        <v-list-item v-for="backup in backups" :key="`${backup.world}:${backup.fileName}`">
          <template #prepend>
            <v-icon size="20" color="on-surface-variant">mdi-archive-outline</v-icon>
          </template>
          <v-list-item-title class="text-body-medium">
            {{ backup.displayName }}
            <v-chip size="x-small" variant="tonal" class="ml-2">{{ kindLabel(backup.kind) }}</v-chip>
          </v-list-item-title>
          <v-list-item-subtitle class="text-body-small">
            {{ backup.world }} · {{ formatTimeAgo(backup.createdAt) }} · {{ formatFileSize(backup.size) }}
          </v-list-item-subtitle>
          <template #append>
            <div class="d-flex ga-1">
              <v-btn
                size="small"
                variant="tonal"
                color="primary"
                prepend-icon="mdi-history"
                :loading="busyKey === `restore:${backup.world}:${backup.fileName}`"
                :disabled="busyKey !== null"
                @click="confirmRestore(backup)"
              >
                {{ t('saves.restore') }}
              </v-btn>
              <v-btn
                size="small"
                variant="text"
                color="error"
                prepend-icon="mdi-delete-outline"
                :loading="busyKey === `delete:${backup.world}:${backup.fileName}`"
                :disabled="busyKey !== null"
                @click="confirmDelete(backup)"
              >
                {{ t('saves.delete') }}
              </v-btn>
            </div>
          </template>
        </v-list-item>
      </v-list>
      <div v-if="backups.length === 0" class="text-center pa-8 text-body-medium text-on-surface-variant">
        {{ t('saves.noBackups') }}
      </div>
      <template v-else>
        <v-divider />
        <div class="text-body-small text-on-surface-variant pa-3">
          {{ t('saves.backupCount', { count: backups.length }) }}
        </div>
      </template>
    </v-card>
  </div>
</template>

<style scoped></style>
