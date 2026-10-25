<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { filterLogLines, parseLogLines, type LogFilter, type LogLevel } from '../../utils/logViewer';

const props = withDefaults(
  defineProps<{
    lines: string[];
    truncated?: boolean;
    loading?: boolean;
    error?: string;
    emptyText?: string;
    emptyHint?: string;
    showFilters?: boolean;
  }>(),
  {
    truncated: false,
    loading: false,
    error: '',
    emptyText: '',
    emptyHint: '',
    showFilters: true,
  },
);

const { t } = useI18n();
const filter = ref<LogFilter>('all');
const query = ref('');

const parsed = computed(() => parseLogLines(props.lines));
const visible = computed(() => filterLogLines(parsed.value, filter.value, query.value));
const hasLines = computed(() => props.lines.length > 0);

function levelClass(level: LogLevel | null): string {
  return level ? `level-${level}` : '';
}
</script>

<template>
  <div class="log-viewer">
    <div v-if="showFilters && hasLines" class="d-flex align-center flex-wrap ga-2 mb-2">
      <v-btn-toggle v-model="filter" density="compact" variant="tonal" mandatory divided>
        <v-btn value="all" size="small">{{ t('logs.filterAll') }}</v-btn>
        <v-btn value="warn" size="small">{{ t('logs.filterWarn') }}</v-btn>
        <v-btn value="error" size="small">{{ t('logs.filterError') }}</v-btn>
      </v-btn-toggle>
      <v-text-field
        v-model="query"
        density="compact"
        hide-details
        clearable
        :placeholder="t('logs.searchPlaceholder')"
        prepend-inner-icon="mdi-magnify"
        style="max-width: 280px"
      />
      <span class="text-body-small text-on-surface-variant">
        {{ t('logs.lineCount', { count: visible.length }) }}
      </span>
      <v-spacer />
      <span v-if="truncated" class="text-body-small text-on-surface-variant">
        {{ t('logs.truncatedHint') }}
      </span>
      <slot name="actions" />
    </div>

    <div v-if="loading" class="d-flex align-center justify-center pa-10">
      <v-progress-circular indeterminate size="28" color="primary" />
    </div>
    <v-alert v-else-if="error" type="error" variant="tonal" density="compact">
      {{ error }}
    </v-alert>
    <div v-else-if="!hasLines" class="text-center pa-10 text-on-surface-variant">
      <v-icon size="40" class="mb-2" color="on-surface-variant">mdi-text-box-outline</v-icon>
      <div class="text-body-medium">{{ emptyText }}</div>
      <div v-if="emptyHint" class="text-body-small mt-1">{{ emptyHint }}</div>
    </div>
    <div v-else-if="visible.length === 0" class="text-center pa-10 text-body-medium text-on-surface-variant">
      {{ t('logs.emptyFiltered') }}
    </div>
    <v-virtual-scroll v-else class="log-scroll" :items="visible" :item-height="22" height="480">
      <template #default="{ item }">
        <div class="log-line" :class="levelClass(item.ownerLevel)">
          <span class="log-no">{{ item.lineNo }}</span>
          <span class="log-text">{{ item.text }}</span>
        </div>
      </template>
    </v-virtual-scroll>
  </div>
</template>

<style scoped>
.log-scroll {
  overflow-x: auto;
}

.log-line {
  display: flex;
  gap: 8px;
  width: max-content;
  min-width: 100%;
  font-family: 'Cascadia Mono', Consolas, monospace;
  font-size: 12px;
  line-height: 22px;
  white-space: pre;
}

.log-no {
  flex: 0 0 56px;
  text-align: right;
  opacity: 0.5;
  user-select: none;
}

.log-text {
  padding-right: 16px;
}

.level-error .log-text,
.level-fatal .log-text {
  color: rgb(var(--v-theme-error));
}

.level-warn .log-text {
  color: rgb(var(--v-theme-warning));
}

.level-debug .log-text,
.level-trace .log-text {
  opacity: 0.65;
}
</style>
