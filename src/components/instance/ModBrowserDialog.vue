<script setup lang="ts">
/**
 * 模组浏览与安装对话框
 * 只按当前实例的 MC 版本与加载器搜索，安装时后端会顺带补齐 required 依赖
 */
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { modApi } from '../../services';
import { useNotificationStore } from '../../stores/notificationStore';
import { useDebounceFn } from '../../composables/useDebounce';
import { getErrorMessage } from '../../utils/format';
import { logError } from '../../utils/logger';
import type { ModrinthModpack } from '../../types/events';

const props = defineProps<{
  modelValue: boolean;
  instanceName: string;
  gameVersion?: string | null;
  loader?: string | null;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void;
  (e: 'installed'): void;
}>();

const notificationStore = useNotificationStore();
const { t } = useI18n();

const query = ref('');
const results = ref<ModrinthModpack[]>([]);
const loading = ref(false);
const error = ref('');
const page = ref(1);
const totalHits = ref(0);
const installing = ref<string | null>(null);
const installedSlugs = ref<Set<string>>(new Set());
const PAGE_SIZE = 10;

const totalPages = computed(() => Math.ceil(totalHits.value / PAGE_SIZE));

// 防抖挡不住「翻页 + 输入」并发的慢响应，旧响应后到会覆盖新结果，按发起顺序丢弃过期响应
let searchSeq = 0;

async function search() {
  if (!props.modelValue) return;

  const seq = ++searchSeq;
  loading.value = true;
  error.value = '';
  try {
    const result = await modApi.searchModrinthMods(props.instanceName, {
      query: query.value.trim() || undefined,
      limit: PAGE_SIZE,
      offset: (page.value - 1) * PAGE_SIZE,
      sortBy: 'relevance',
    });
    if (seq !== searchSeq) return;
    results.value = result.hits || [];
    totalHits.value = result.total_hits || 0;
  } catch (e) {
    if (seq !== searchSeq) return;
    logError('Failed to search mods', e, 'ModBrowserDialog');
    error.value = getErrorMessage(e);
    results.value = [];
    totalHits.value = 0;
  } finally {
    if (seq === searchSeq) {
      loading.value = false;
    }
  }
}

const searchDebounced = useDebounceFn(() => {
  page.value = 1;
  void search();
}, 300);

watch(query, () => searchDebounced.call());

watch(page, () => {
  void search();
});

watch(
  () => props.modelValue,
  (open) => {
    if (open) {
      results.value = [];
      installedSlugs.value = new Set();
      page.value = 1;
      void search();
    }
  },
);

async function install(mod: ModrinthModpack) {
  if (installing.value) return;

  installing.value = mod.slug;
  try {
    const result = await modApi.installModrinthMod(props.instanceName, mod.slug, undefined, true);
    const names = result.installed.map((item) => item.name);
    const name = names[0] ?? mod.title;
    notificationStore.success(
      t('modBrowser.installSuccess'),
      names.length > 1
        ? t('modBrowser.installSuccessWithDeps', { name, count: names.length - 1 }, names.length - 1)
        : name,
    );
    installedSlugs.value = new Set([...installedSlugs.value, mod.slug]);
    emit('installed');
  } catch (e) {
    logError('Failed to install mod', e, 'ModBrowserDialog');
    notificationStore.error(t('modBrowser.installFailed'), getErrorMessage(e), true);
  } finally {
    installing.value = null;
  }
}

function formatNumber(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(value);
}

const ALLOWED_IMAGE_HOSTS = new Set(['cdn.modrinth.com', 'assets.modrinth.com']);

function iconUrl(url: string | undefined): string {
  if (!url) return '';
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'https:') return '';
    return ALLOWED_IMAGE_HOSTS.has(parsed.hostname) ? url : '';
  } catch {
    return '';
  }
}
</script>

<template>
  <v-dialog
    :model-value="modelValue"
    max-width="720"
    scrollable
    @update:model-value="emit('update:modelValue', $event)"
  >
    <v-card color="surface-container-high">
      <v-card-title class="d-flex align-center pa-4 pb-2">
        <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-plus-box-outline</v-icon>
        <div class="flex-grow-1">
          <div class="text-title-medium font-weight-bold">{{ t('modBrowser.title') }}</div>
          <div class="text-body-small text-on-surface-variant">
            {{ instanceName }}
            <template v-if="gameVersion"> · {{ gameVersion }}</template>
            <template v-if="loader"> · {{ loader }}</template>
          </div>
        </div>
        <v-btn
          icon
          size="small"
          variant="text"
          :aria-label="t('modBrowser.close')"
          @click="emit('update:modelValue', false)"
        >
          <v-icon size="18">mdi-close</v-icon>
        </v-btn>
      </v-card-title>

      <v-card-text class="pa-4 pb-2">
        <v-text-field
          v-model="query"
          density="comfortable"
          variant="outlined"
          :label="t('modBrowser.searchLabel')"
          :placeholder="t('modBrowser.searchPlaceholder')"
          hide-details
          clearable
        >
          <template #prepend-inner>
            <v-icon size="20" color="on-surface-variant">mdi-magnify</v-icon>
          </template>
        </v-text-field>
      </v-card-text>

      <v-divider />

      <v-card-text class="pa-0" style="min-height: 200px">
        <div v-if="loading" class="d-flex align-center justify-center pa-10">
          <v-progress-circular indeterminate size="28" color="primary" />
        </div>

        <div v-else-if="error" class="text-center pa-8">
          <div class="text-body-medium text-error mb-3">{{ error }}</div>
          <v-btn size="small" variant="tonal" color="primary" @click="search">{{ t('modBrowser.retry') }}</v-btn>
        </div>

        <div v-else-if="results.length === 0" class="text-center pa-10 text-on-surface-variant">
          <v-icon size="36" class="mb-2">mdi-puzzle-outline</v-icon>
          <div class="text-body-medium">{{ t('modBrowser.empty') }}</div>
        </div>

        <v-list v-else lines="two" density="compact">
          <v-list-item v-for="mod in results" :key="mod.slug">
            <template #prepend>
              <v-avatar size="40" rounded="lg" class="mr-3" color="secondary-container">
                <v-img v-if="iconUrl(mod.icon_url)" :src="iconUrl(mod.icon_url)" cover>
                  <template #error>
                    <v-icon size="20" color="on-secondary-container">mdi-puzzle-outline</v-icon>
                  </template>
                </v-img>
                <v-icon v-else size="20" color="on-secondary-container">mdi-puzzle-outline</v-icon>
              </v-avatar>
            </template>

            <v-list-item-title class="text-body-medium font-weight-medium">
              {{ mod.title }}
            </v-list-item-title>
            <v-list-item-subtitle class="text-body-small mod-desc">
              {{ mod.description }}
            </v-list-item-subtitle>

            <template #append>
              <div class="d-flex align-center ga-2">
                <span class="text-body-small text-on-surface-variant d-none d-sm-inline">
                  <v-icon size="14" class="mr-1">mdi-download</v-icon>{{ formatNumber(mod.downloads) }}
                </span>
                <v-btn
                  size="small"
                  variant="tonal"
                  color="primary"
                  :loading="installing === mod.slug"
                  :disabled="installing !== null || installedSlugs.has(mod.slug)"
                  @click="install(mod)"
                >
                  {{ installedSlugs.has(mod.slug) ? t('modBrowser.installed') : t('modBrowser.install') }}
                </v-btn>
              </div>
            </template>
          </v-list-item>
        </v-list>
      </v-card-text>

      <v-divider v-if="totalPages > 1" />
      <v-card-actions v-if="totalPages > 1" class="pa-2">
        <span class="text-body-small text-on-surface-variant ml-2">
          {{ t('modBrowser.totalHits', { count: totalHits }, totalHits) }}
        </span>
        <v-spacer />
        <v-pagination v-model="page" :length="totalPages" :total-visible="5" density="compact" color="primary" />
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>

<style scoped>
.mod-desc {
  display: -webkit-box;
  -webkit-line-clamp: 1;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
</style>
