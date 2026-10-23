/**
 * Modrinth 整合包搜索
 * 搜索词与版本、加载器、分类、排序联动加分页，输入变化 300ms 防抖
 */
import { ref, computed, watch } from 'vue';
import { api } from '../services';
import { translate } from '../i18n';
import { useDebounceFn } from './useDebounce';
import { logError } from '../utils/logger';
import { compareVersionDesc } from '../utils/format';
import type { ModrinthModpack } from '../types/events';

export function useModrinth() {
  const modpackSearchQuery = ref('');
  const selectedGameVersion = ref<string | null>(null);
  const selectedLoader = ref<string | null>(null);
  const selectedCategory = ref<string | null>(null);
  const sortBy = ref('relevance');
  const modpacks = ref<ModrinthModpack[]>([]);
  const loadingModpacks = ref(false);
  const modpackCurrentPage = ref(1);
  const modpackItemsPerPage = 12;
  const modpackTotalHits = ref(0);
  const modpackInitialLoad = ref(true);

  const modpackTotalPages = computed(() => Math.ceil(modpackTotalHits.value / modpackItemsPerPage));

  const gameVersions = ref<string[]>([]);
  const loaders = ['fabric', 'forge', 'quilt', 'neoforge'];
  // value 是 Modrinth 的英文分类 id，title 只用于界面展示
  // 只能用 project_type=modpack 的那一组分类，模组分类（decoration、worldgen 之类）拿去筛整合包查不出东西
  const categoryOptions = computed(() => [
    { title: translate('modpackBrowser.categories.adventure'), value: 'adventure' },
    { title: translate('modpackBrowser.categories.challenging'), value: 'challenging' },
    { title: translate('modpackBrowser.categories.combat'), value: 'combat' },
    { title: translate('modpackBrowser.categories.kitchenSink'), value: 'kitchen-sink' },
    { title: translate('modpackBrowser.categories.lightweight'), value: 'lightweight' },
    { title: translate('modpackBrowser.categories.magic'), value: 'magic' },
    { title: translate('modpackBrowser.categories.multiplayer'), value: 'multiplayer' },
    { title: translate('modpackBrowser.categories.optimization'), value: 'optimization' },
    { title: translate('modpackBrowser.categories.quests'), value: 'quests' },
    { title: translate('modpackBrowser.categories.technology'), value: 'technology' },
  ]);
  const modpackSortOptions = computed(() => [
    { title: translate('modpackBrowser.sort.relevance'), value: 'relevance' },
    { title: translate('modpackBrowser.sort.downloads'), value: 'downloads' },
    { title: translate('modpackBrowser.sort.follows'), value: 'follows' },
    { title: translate('modpackBrowser.sort.newest'), value: 'newest' },
    { title: translate('modpackBrowser.sort.updated'), value: 'updated' },
  ]);

  async function fetchGameVersions() {
    try {
      const manifest = await api.version.getVersions();
      gameVersions.value = manifest.versions
        .filter((v) => v.type === 'release')
        .map((v) => v.id)
        .sort(compareVersionDesc);
    } catch (error) {
      logError('Failed to fetch game versions for filter', error, 'useModrinth');
    }
  }

  // 筛选立即搜索与搜索词防抖搜索可并发，慢的旧响应后到会覆盖新结果
  let modpackSearchSeq = 0;

  async function searchModpacksInternal() {
    const seq = ++modpackSearchSeq;
    loadingModpacks.value = true;
    try {
      const offset = (modpackCurrentPage.value - 1) * modpackItemsPerPage;

      const result = await api.modpack.searchModrinthModpacks({
        query: modpackSearchQuery.value || undefined,
        gameVersions: selectedGameVersion.value ? [selectedGameVersion.value] : undefined,
        loaders: selectedLoader.value ? [selectedLoader.value] : undefined,
        categories: selectedCategory.value ? [selectedCategory.value] : undefined,
        limit: modpackItemsPerPage,
        offset: offset,
        sortBy: sortBy.value,
      });

      if (seq !== modpackSearchSeq) return;
      modpacks.value = result.hits || [];
      modpackTotalHits.value = result.total_hits || 0;
      modpackInitialLoad.value = false;
    } catch (error) {
      if (seq !== modpackSearchSeq) return;
      logError('Failed to search modpacks', error, 'useModrinth');
      modpacks.value = [];
      modpackTotalHits.value = 0;
      modpackInitialLoad.value = false;
    } finally {
      if (seq === modpackSearchSeq) {
        loadingModpacks.value = false;
      }
    }
  }

  const debouncedSearch = useDebounceFn(searchModpacksInternal, 300);

  async function searchModpacks() {
    await searchModpacksInternal();
  }

  function searchModpacksDebounced() {
    debouncedSearch.call();
  }

  watch(modpackSearchQuery, () => {
    if (!modpackInitialLoad.value) {
      modpackCurrentPage.value = 1;
      searchModpacksDebounced();
    }
  });

  watch([selectedGameVersion, selectedLoader, selectedCategory, sortBy], () => {
    if (!modpackInitialLoad.value) {
      modpackCurrentPage.value = 1;
      searchModpacks();
    }
  });

  async function onModpackPageChange(page: number) {
    modpackCurrentPage.value = page;
    await searchModpacks();
  }

  return {
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
    searchModpacksDebounced,
    onModpackPageChange,
  };
}
