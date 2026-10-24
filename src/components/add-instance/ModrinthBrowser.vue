<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { useModrinth } from '../../composables/useModrinth';
import { curseforgeApi } from '../../services';
import { logError } from '../../utils/logger';
import type { ModrinthModpack } from '../../types/events';
import ModpackCard from './ModpackCard.vue';

const props = defineProps<{
  /** 数据源由页面级的平台切换决定 */
  source: 'modrinth' | 'curseforge';
}>();

const router = useRouter();
const { t } = useI18n();
const selectedModpack = ref<ModrinthModpack | null>(null);

const {
  modpackSearchQuery,
  selectedGameVersion,
  selectedLoader,
  selectedCategory,
  sortBy,
  modpacks,
  loadingModpacks,
  modpackCurrentPage,
  modpackTotalHits,
  modpackTotalPages,
  modpackInitialLoad,
  gameVersions,
  loaders,
  categoryOptions,
  modpackSortOptions,
  fetchGameVersions,
  searchModpacks,
  onModpackPageChange,
} = useModrinth(() => props.source);

// 搜索框里是 CurseForge 链接或纯数字 ID 时，给出绕过搜索库的直接安装入口。
// minekuai 的项目库收录不全（新包搜不到但 versions 端点有数据），
// 解析出数字 ID 后照常进版本列表与安装流程
const directRefInput = computed(() => {
  if (props.source !== 'curseforge') return null;
  const text = modpackSearchQuery.value?.trim();
  if (!text) return null;
  if (/^\d{2,10}$/.test(text)) return text;
  if (/^https?:\/\/([a-z0-9-]+\.)*curseforge\.com\//i.test(text)) return text;
  return null;
});
const resolvingDirectRef = ref(false);
const directRefError = ref('');

async function installFromDirectRef() {
  if (!directRefInput.value || resolvingDirectRef.value) return;
  resolvingDirectRef.value = true;
  directRefError.value = '';
  try {
    const resolved = await curseforgeApi.resolveModpackRef(directRefInput.value);
    router.push({
      path: '/install-modpack',
      query: {
        projectId: resolved.projectId,
        title: resolved.title,
        source: 'curseforge',
      },
    });
  } catch (error) {
    directRefError.value = String(error);
    logError('Failed to resolve CurseForge modpack ref', error, 'ModrinthBrowser');
  } finally {
    resolvingDirectRef.value = false;
  }
}

// 两个数据源的搜索参数语义一致，切源重新搜一轮；
// 分类 slug 两套 taxonomy 互不相通（Modrinth 的 technology 在 CF 叫 tech），
// 切源时清掉旧分类，否则残留值会把结果过滤成 0；
// follows 是 Modrinth 独有排序，一并重置。筛选联动可能额外触发一次搜索，序号守卫会丢弃过期响应
watch(
  () => props.source,
  () => {
    selectedCategory.value = null;
    if (props.source === 'curseforge' && sortBy.value === 'follows') {
      sortBy.value = 'relevance';
    }
    modpackCurrentPage.value = 1;
    void searchModpacks();
  },
);

function selectModpack(modpack: ModrinthModpack) {
  selectedModpack.value = modpack;
  router.push({
    path: '/install-modpack',
    query: {
      projectId: modpack.slug,
      title: modpack.title,
      source: props.source,
    },
  });
}

onMounted(async () => {
  await fetchGameVersions();
  await searchModpacks();
});
</script>

<template>
  <div>
    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <v-row density="compact">
          <v-col cols="12" sm="6">
            <v-text-field
              density="comfortable"
              v-model="modpackSearchQuery"
              :label="t('modpackBrowser.searchLabel')"
              hide-details
              clearable
            >
              <template #prepend-inner>
                <v-icon size="20" color="on-surface-variant">mdi-magnify</v-icon>
              </template>
            </v-text-field>
          </v-col>
          <v-col cols="6" sm="3">
            <v-select
              density="comfortable"
              v-model="selectedGameVersion"
              :items="gameVersions"
              :label="t('modpackBrowser.gameVersionLabel')"
              clearable
              hide-details
            >
              <template #prepend-inner>
                <v-icon size="20" color="on-surface-variant">mdi-minecraft</v-icon>
              </template>
            </v-select>
          </v-col>
          <v-col cols="6" sm="3">
            <v-select
              density="comfortable"
              v-model="selectedLoader"
              :items="loaders"
              :label="t('modpackBrowser.loaderLabel')"
              clearable
              hide-details
            >
              <template #prepend-inner>
                <v-icon size="20" color="on-surface-variant">mdi-puzzle</v-icon>
              </template>
            </v-select>
          </v-col>
        </v-row>

        <v-row density="compact" class="mt-2">
          <v-col cols="6">
            <v-select
              density="comfortable"
              v-model="selectedCategory"
              :items="categoryOptions"
              :label="t('modpackBrowser.categoryLabel')"
              clearable
              hide-details
            >
              <template #prepend-inner>
                <v-icon size="20" color="on-surface-variant">mdi-tag</v-icon>
              </template>
            </v-select>
          </v-col>
          <v-col cols="6">
            <v-select
              density="comfortable"
              v-model="sortBy"
              :items="modpackSortOptions"
              :label="t('modpackBrowser.sortLabel')"
              hide-details
            >
              <template #prepend-inner>
                <v-icon size="20" color="on-surface-variant">mdi-sort</v-icon>
              </template>
            </v-select>
          </v-col>
        </v-row>
      </v-card-text>
    </v-card>

    <v-card v-if="directRefInput" variant="tonal" color="primary" class="mb-4" data-testid="cf-direct-install">
      <v-card-text class="d-flex align-center flex-wrap ga-3">
        <v-icon color="primary">mdi-link-variant</v-icon>
        <div class="flex-grow-1 min-width-0">
          <div class="text-body-medium font-weight-medium">
            {{ t('modpackBrowser.directInstallTitle') }}
          </div>
          <div class="text-body-small text-on-surface-variant">
            {{ t('modpackBrowser.directInstallHint') }}
          </div>
          <div v-if="directRefError" class="text-body-small text-error mt-1">
            {{ t('modpackBrowser.directInstallFailed') }}
          </div>
        </div>
        <v-btn color="primary" variant="tonal" :loading="resolvingDirectRef" @click="installFromDirectRef">
          {{ t('modpackBrowser.directInstallAction') }}
        </v-btn>
      </v-card-text>
    </v-card>

    <div v-if="loadingModpacks" class="text-center py-12">
      <v-progress-circular indeterminate size="48" color="primary" />
      <div class="text-body-medium text-on-surface-variant mt-4">{{ t('modpackBrowser.searching') }}</div>
    </div>

    <div v-else-if="modpacks.length === 0 && !directRefInput" class="text-center py-12">
      <v-avatar size="80" color="surface-container-high" class="mb-4">
        <v-icon size="40" color="on-surface-variant">mdi-package-variant</v-icon>
      </v-avatar>
      <div class="text-body-large font-weight-medium">
        {{ modpackInitialLoad ? t('modpackBrowser.loading') : t('modpackBrowser.empty') }}
      </div>
      <div class="text-body-medium text-on-surface-variant">
        {{ modpackInitialLoad ? '' : t('modpackBrowser.emptyHint') }}
      </div>
    </div>

    <template v-else>
      <v-row density="compact">
        <v-col v-for="modpack in modpacks" :key="modpack.slug" cols="12" sm="6" md="4">
          <ModpackCard :modpack="modpack" :selected="selectedModpack?.slug === modpack.slug" @select="selectModpack" />
        </v-col>
      </v-row>

      <div v-if="modpackTotalPages > 1" class="d-flex flex-column align-center mt-5">
        <v-pagination
          v-model="modpackCurrentPage"
          :length="modpackTotalPages"
          :total-visible="5"
          density="comfortable"
          color="primary"
          @update:model-value="onModpackPageChange"
        />
        <div class="text-body-small text-on-surface-variant mt-2">
          {{ t('modpackBrowser.totalHits', { count: modpackTotalHits }, modpackTotalHits) }}
        </div>
      </div>
    </template>
  </div>
</template>
