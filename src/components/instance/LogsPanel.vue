<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import LogViewer from './LogViewer.vue';
import { useDiagnosticsExport } from '../../composables/useDiagnosticsExport';
import { useInstanceScope } from '../../composables/useInstanceScope';
import { useNotificationStore } from '../../stores/notificationStore';
import { configApi, logApi } from '../../services';
import type { CrashReportInfo, LaunchOutcome, LaunchRecord, LogTail } from '../../types/events';
import { copyText } from '../../utils/clipboard';
import { formatFileSize, formatTimeAgo, getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';

const props = defineProps<{ instanceName: string }>();
const { t } = useI18n();
const notificationStore = useNotificationStore();
const scope = useInstanceScope(() => props.instanceName);
const { exportingInstance, exportDiagnostics } = useDiagnosticsExport();

type SubTab = 'latest' | 'history' | 'crashes';
const subTab = ref<SubTab>('latest');
const versionIsolation = ref(true);

const latest = ref<LogTail | null>(null);
const history = ref<LaunchRecord[]>([]);
const crashes = ref<CrashReportInfo[]>([]);

const loadingLatest = ref(true);
const loadingHistory = ref(true);
const loadingCrashes = ref(true);
const latestError = ref('');
const historyError = ref('');
const crashesError = ref('');

const logDialog = ref(false);
const logDialogTitle = ref('');
const logDialogLoading = ref(false);
const logDialogError = ref('');
const logDialogTruncated = ref(false);
const logDialogLines = ref<string[]>([]);

const crashDialog = ref(false);
const crashDialogTitle = ref('');
const crashDialogLoading = ref(false);
const crashDialogError = ref('');
const crashDialogTruncated = ref(false);
const crashDialogContent = ref('');
const crashDialogLines = computed(() => (crashDialogContent.value ? crashDialogContent.value.split(/\r?\n/) : []));

const OUTCOMES: Record<LaunchOutcome, { label: string; color: string }> = {
  exited: { label: 'logs.statusExited', color: 'success' },
  crashed: { label: 'logs.statusCrashed', color: 'error' },
  running: { label: 'logs.statusRunning', color: 'grey' },
  unknown: { label: 'logs.statusUnknown', color: 'warning' },
};

const refreshing = computed(() => loadingLatest.value || loadingHistory.value || loadingCrashes.value);

async function loadLatest() {
  const task = scope.begin();
  loadingLatest.value = true;
  latestError.value = '';
  try {
    const result = await logApi.getLatestLog(task.instanceName);
    if (!scope.isCurrent(task)) return;
    latest.value = result;
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load latest log', e, 'LogsPanel');
    latestError.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) loadingLatest.value = false;
  }
}

async function loadHistory() {
  const task = scope.begin();
  loadingHistory.value = true;
  historyError.value = '';
  try {
    const result = await logApi.getHistory(task.instanceName);
    if (!scope.isCurrent(task)) return;
    history.value = result;
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load launch history', e, 'LogsPanel');
    historyError.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) loadingHistory.value = false;
  }
}

async function loadCrashes() {
  const task = scope.begin();
  loadingCrashes.value = true;
  crashesError.value = '';
  try {
    const result = await logApi.getCrashReports(task.instanceName);
    if (!scope.isCurrent(task)) return;
    crashes.value = result;
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load crash reports', e, 'LogsPanel');
    crashesError.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) loadingCrashes.value = false;
  }
}

async function loadIsolation() {
  const task = scope.begin();
  try {
    const value = await configApi.loadConfigKey('versionIsolation');
    if (scope.isCurrent(task)) versionIsolation.value = value !== 'false';
  } catch (e) {
    logError('Failed to read version isolation', e, 'LogsPanel');
  }
}

function loadAll() {
  void loadLatest();
  void loadHistory();
  void loadCrashes();
  void loadIsolation();
}

function closeDialogs() {
  logDialog.value = false;
  crashDialog.value = false;
}

function refreshCurrent() {
  if (subTab.value === 'latest') void loadLatest();
  else if (subTab.value === 'history') void loadHistory();
  else void loadCrashes();
}

async function exportBundle() {
  await exportDiagnostics({ name: props.instanceName });
}

async function openLogsFolder() {
  try {
    await logApi.openContent(props.instanceName, 'logsFolder');
  } catch (e) {
    logError('Failed to open logs folder', e, 'LogsPanel');
    notificationStore.error(t('logs.openFolderFailed'), getErrorMessage(e));
  }
}

async function openLatestLog() {
  try {
    await logApi.openContent(props.instanceName, 'latestLog');
  } catch (e) {
    logError('Failed to open latest log', e, 'LogsPanel');
    notificationStore.error(t('logs.openFolderFailed'), getErrorMessage(e));
  }
}

async function openCrashFolder() {
  try {
    await logApi.openContent(props.instanceName, 'crashReportsFolder');
  } catch (e) {
    logError('Failed to open crash reports folder', e, 'LogsPanel');
    notificationStore.error(t('logs.openCrashFolderFailed'), getErrorMessage(e));
  }
}

async function viewLaunchLog(record: LaunchRecord) {
  logDialog.value = true;
  logDialogTitle.value = t('logs.launchLogTitle', {
    time: formatDateTime(record.startedAt),
  });
  logDialogLoading.value = true;
  logDialogError.value = '';
  logDialogTruncated.value = false;
  logDialogLines.value = [];

  const task = scope.begin();
  try {
    const result = await logApi.getLaunchLog(task.instanceName, record.id);
    if (!scope.isCurrent(task)) return;
    logDialogLines.value = result.lines;
    logDialogTruncated.value = result.truncated;
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load launch log', e, 'LogsPanel');
    logDialogError.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) logDialogLoading.value = false;
  }
}

async function viewCrashReport(report: CrashReportInfo) {
  crashDialog.value = true;
  crashDialogTitle.value = t('logs.crashReportTitle', { name: report.fileName });
  crashDialogLoading.value = true;
  crashDialogError.value = '';
  crashDialogTruncated.value = false;
  crashDialogContent.value = '';

  const task = scope.begin();
  try {
    const result = await logApi.getCrashReport(task.instanceName, report.fileName);
    if (!scope.isCurrent(task)) return;
    crashDialogContent.value = result.content;
    crashDialogTruncated.value = result.truncated;
  } catch (e) {
    if (!scope.isCurrent(task)) return;
    logError('Failed to load crash report', e, 'LogsPanel');
    crashDialogError.value = getErrorMessage(e);
  } finally {
    if (scope.isCurrent(task)) crashDialogLoading.value = false;
  }
}

async function copyCrashReport() {
  if (await copyText(crashDialogContent.value)) {
    notificationStore.info(t('common.copied'));
  } else {
    notificationStore.error(t('logs.copyFailed'));
  }
}

function formatDuration(seconds: number | null): string {
  if (seconds === null) return '—';
  const total = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  const pad = (value: number) => String(value).padStart(2, '0');
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${minutes}:${pad(rest)}`;
}

function formatDateTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString();
}

onMounted(loadAll);
watch(subTab, (tab) => {
  if (tab === 'latest') void loadLatest();
  else if (tab === 'history') void loadHistory();
  else void loadCrashes();
});
watch(
  () => props.instanceName,
  () => {
    closeDialogs();
    loadAll();
  },
);
</script>

<template>
  <div class="logs-panel">
    <div class="d-flex align-center ga-2 mb-3 flex-wrap">
      <v-btn
        color="primary"
        variant="tonal"
        size="small"
        prepend-icon="mdi-download"
        :loading="exportingInstance !== null"
        :disabled="exportingInstance !== null"
        @click="exportBundle"
      >
        {{ t('logs.exportDiagnostics') }}
      </v-btn>
      <v-spacer />
      <v-btn
        icon
        size="small"
        variant="text"
        :aria-label="t('logs.refresh')"
        :loading="refreshing"
        @click="refreshCurrent"
      >
        <v-icon size="18">mdi-refresh</v-icon>
      </v-btn>
    </div>

    <v-alert v-if="!versionIsolation" type="warning" variant="tonal" density="comfortable" class="mb-3">
      {{ t('logs.isolationWarning') }}
    </v-alert>

    <v-tabs v-model="subTab" color="primary" class="mb-3">
      <v-tab value="latest">{{ t('logs.tabLatest') }}</v-tab>
      <v-tab value="history">{{ t('logs.tabHistory') }}</v-tab>
      <v-tab value="crashes">{{ t('logs.tabCrashes') }}</v-tab>
    </v-tabs>

    <v-window v-model="subTab">
      <v-window-item value="latest">
        <v-card variant="flat" color="surface-container">
          <v-card-text class="pa-4">
            <LogViewer
              :lines="latest?.lines ?? []"
              :truncated="latest?.truncated ?? false"
              :loading="loadingLatest"
              :error="latestError"
              :empty-text="t('logs.emptyLatest')"
              :empty-hint="t('logs.emptyLatestHint')"
            >
              <template #actions>
                <v-btn size="small" variant="text" prepend-icon="mdi-file-find-outline" @click="openLatestLog">
                  {{ t('logs.openOriginal') }}
                </v-btn>
              </template>
            </LogViewer>
          </v-card-text>
          <v-divider />
          <div class="d-flex align-center flex-wrap ga-2 text-body-small text-on-surface-variant pa-3">
            <span>{{ t('logs.latestHint') }}</span>
            <v-spacer />
            <v-btn size="x-small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openLogsFolder">
              {{ t('logs.openFolder') }}
            </v-btn>
          </div>
        </v-card>
      </v-window-item>

      <v-window-item value="history">
        <v-card variant="flat" color="surface-container">
          <div v-if="loadingHistory && history.length === 0" class="d-flex align-center justify-center pa-10">
            <v-progress-circular indeterminate size="28" color="primary" />
          </div>

          <div v-else-if="historyError" class="text-center pa-8">
            <div class="text-body-medium text-error mb-3">{{ historyError }}</div>
            <v-btn size="small" variant="tonal" color="primary" @click="loadHistory">
              {{ t('common.retry') }}
            </v-btn>
          </div>

          <div v-else-if="history.length === 0" class="text-center pa-10 text-on-surface-variant">
            <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-history</v-icon>
            <div class="text-body-medium">{{ t('logs.emptyHistory') }}</div>
            <div class="text-body-small mt-1">{{ t('logs.emptyHistoryHint') }}</div>
          </div>

          <template v-else>
            <v-list density="compact" lines="two" bg-color="transparent">
              <v-list-item v-for="record in history" :key="record.id">
                <template #prepend>
                  <v-icon size="20" color="on-surface-variant">mdi-history</v-icon>
                </template>
                <v-list-item-title class="d-flex align-center flex-wrap ga-2 text-body-medium">
                  {{ formatDateTime(record.startedAt) }}
                  <v-chip size="x-small" variant="tonal" :color="OUTCOMES[record.outcome].color">
                    {{ t(OUTCOMES[record.outcome].label) }}
                  </v-chip>
                  <v-chip v-if="record.crashReport" size="x-small" variant="tonal" color="error">
                    {{ t('logs.hasCrashReport') }}
                  </v-chip>
                </v-list-item-title>
                <v-list-item-subtitle class="text-body-small">
                  {{ t('logs.duration') }} {{ formatDuration(record.durationSecs) }}
                  <template v-if="record.exitCode !== null">
                    · {{ t('logs.exitCode', { code: record.exitCode }) }}
                  </template>
                </v-list-item-subtitle>
                <template #append>
                  <v-btn
                    size="small"
                    variant="tonal"
                    color="primary"
                    prepend-icon="mdi-text-box-search-outline"
                    :disabled="record.outcome === 'running'"
                    @click="viewLaunchLog(record)"
                  >
                    {{ t('logs.viewLog') }}
                  </v-btn>
                </template>
              </v-list-item>
            </v-list>
            <v-divider />
            <div class="d-flex align-center flex-wrap ga-2 text-body-small text-on-surface-variant pa-3">
              <span>{{ t('logs.historyCount', { count: history.length }) }}</span>
              <v-spacer />
              <v-btn size="x-small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openLogsFolder">
                {{ t('logs.openFolder') }}
              </v-btn>
            </div>
          </template>
        </v-card>
      </v-window-item>

      <v-window-item value="crashes">
        <v-card variant="flat" color="surface-container">
          <div v-if="loadingCrashes && crashes.length === 0" class="d-flex align-center justify-center pa-10">
            <v-progress-circular indeterminate size="28" color="primary" />
          </div>

          <div v-else-if="crashesError" class="text-center pa-8">
            <div class="text-body-medium text-error mb-3">{{ crashesError }}</div>
            <v-btn size="small" variant="tonal" color="primary" @click="loadCrashes">
              {{ t('common.retry') }}
            </v-btn>
          </div>

          <div v-else-if="crashes.length === 0" class="text-center pa-10 text-on-surface-variant">
            <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-alert-circle-outline</v-icon>
            <div class="text-body-medium">{{ t('logs.emptyCrashes') }}</div>
            <div class="text-body-small mt-1">{{ t('logs.emptyCrashesHint') }}</div>
          </div>

          <template v-else>
            <v-list density="compact" lines="two" bg-color="transparent">
              <v-list-item v-for="report in crashes" :key="report.fileName">
                <template #prepend>
                  <v-icon size="20" color="on-surface-variant">mdi-alert-circle-outline</v-icon>
                </template>
                <v-list-item-title class="text-body-medium">{{ report.fileName }}</v-list-item-title>
                <v-list-item-subtitle class="text-body-small">
                  {{ formatTimeAgo(report.modifiedAt) }} · {{ formatFileSize(report.size) }}
                </v-list-item-subtitle>
                <template #append>
                  <v-btn
                    size="small"
                    variant="tonal"
                    color="primary"
                    prepend-icon="mdi-text-box-search-outline"
                    @click="viewCrashReport(report)"
                  >
                    {{ t('logs.viewReport') }}
                  </v-btn>
                </template>
              </v-list-item>
            </v-list>
            <v-divider />
            <div class="d-flex align-center flex-wrap ga-2 text-body-small text-on-surface-variant pa-3">
              <span>{{ t('logs.crashCount', { count: crashes.length }) }}</span>
              <v-spacer />
              <v-btn size="x-small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openCrashFolder">
                {{ t('logs.openCrashFolder') }}
              </v-btn>
            </div>
          </template>
        </v-card>
      </v-window-item>
    </v-window>

    <v-dialog v-model="logDialog" max-width="1100" scrollable>
      <v-card color="surface-container-high">
        <v-card-title class="d-flex align-center flex-wrap ga-2 pa-4 pb-2">
          <div class="text-title-large font-weight-bold flex-grow-1">{{ logDialogTitle }}</div>
          <v-btn icon variant="text" :aria-label="t('common.close')" @click="logDialog = false">
            <v-icon>mdi-close</v-icon>
          </v-btn>
        </v-card-title>
        <v-card-text class="pa-4 pt-2">
          <LogViewer
            :lines="logDialogLines"
            :truncated="logDialogTruncated"
            :loading="logDialogLoading"
            :error="logDialogError"
            :empty-text="t('logs.emptyFile')"
          />
        </v-card-text>
      </v-card>
    </v-dialog>

    <v-dialog v-model="crashDialog" max-width="1100" scrollable>
      <v-card color="surface-container-high">
        <v-card-title class="d-flex align-center flex-wrap ga-2 pa-4 pb-2">
          <div class="text-title-large font-weight-bold flex-grow-1">{{ crashDialogTitle }}</div>
          <v-btn
            size="small"
            variant="text"
            prepend-icon="mdi-content-copy"
            :disabled="!crashDialogContent"
            @click="copyCrashReport"
          >
            {{ t('common.copy') }}
          </v-btn>
          <v-btn size="small" variant="text" prepend-icon="mdi-folder-open-outline" @click="openCrashFolder">
            {{ t('logs.openCrashFolder') }}
          </v-btn>
          <v-btn icon variant="text" :aria-label="t('common.close')" @click="crashDialog = false">
            <v-icon>mdi-close</v-icon>
          </v-btn>
        </v-card-title>
        <v-card-text class="pa-4 pt-2">
          <v-alert v-if="crashDialogTruncated" type="info" variant="tonal" density="compact" class="mb-2">
            {{ t('logs.truncatedFile') }}
          </v-alert>
          <LogViewer
            :lines="crashDialogLines"
            :loading="crashDialogLoading"
            :error="crashDialogError"
            :empty-text="t('logs.emptyFile')"
            :show-filters="false"
          />
        </v-card-text>
      </v-card>
    </v-dialog>
  </div>
</template>
