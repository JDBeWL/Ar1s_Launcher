/**
 * 整合包搜索
 * 搜索词与版本、加载器、分类、排序联动加分页，输入变化 300ms 防抖；
 * source 切换数据源：Modrinth 官方 API 或 CurseForge（麦块开放 API 中转），
 * 传 getter 保持响应式——平台切换后搜索与分类选项都要跟着换
 */
import { ref, computed, toValue, watch } from 'vue';
import type { MaybeRefOrGetter } from 'vue';
import { api } from '../services';
import { translate } from '../i18n';
import { useDebounceFn } from './useDebounce';
import { logError } from '../utils/logger';
import { compareVersionDesc } from '../utils/format';
import type { ModrinthModpack } from '../types/events';

export type ModpackSource = 'modrinth' | 'curseforge';

/** 已知的 CF 分类给出中文，接口新出现的用上游英文名兜底 */
function cfCategoryTitle(slug: string, name: string): string {
  const key = `modpackBrowser.cfCategories.${slug}`;
  const translated = translate(key);
  return translated === key ? name : translated;
}

export function useModrinth(source: MaybeRefOrGetter<ModpackSource> = 'modrinth') {
  const sourceRef = computed(() => toValue(source));
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
  const modrinthCategoryOptions = computed(() => [
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

  // CF 与 Modrinth 的分类 slug 完全不同（Modrinth 的 technology 在 CF 叫 tech），
  // 选项列表按源切换；CF 的从麦块 API 的 categories 端点拉取，懒加载一次
  const cfCategoryOptions = ref<{ title: string; value: string }[]>([]);

  const categoryOptions = computed(() =>
    sourceRef.value === 'curseforge' ? cfCategoryOptions.value : modrinthCategoryOptions.value,
  );

  async function ensureCfCategories() {
    if (cfCategoryOptions.value.length > 0) return;
    try {
      const list = await api.curseforge.getModpackCategories();
      cfCategoryOptions.value = list.map((category) => ({
        title: cfCategoryTitle(category.slug, category.name),
        value: category.slug,
      }));
    } catch (error) {
      logError('Failed to fetch CurseForge categories', error, 'useModrinth');
    }
  }

  const modpackSortOptions = computed(() => {
    const base = [
      { title: translate('modpackBrowser.sort.relevance'), value: 'relevance' },
      { title: translate('modpackBrowser.sort.downloads'), value: 'downloads' },
      { title: translate('modpackBrowser.sort.newest'), value: 'newest' },
      { title: translate('modpackBrowser.sort.updated'), value: 'updated' },
    ];
    // CurseForge 不提供收藏数维度
    if (sourceRef.value === 'modrinth') {
      base.splice(2, 0, { title: translate('modpackBrowser.sort.follows'), value: 'follows' });
    }
    return base;
  });

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

    if (sourceRef.value === 'curseforge') {
      // 分类选项懒加载：不阻塞搜索，拉回来后下拉框自然更新
      void ensureCfCategories();
    }

    try {
      const offset = (modpackCurrentPage.value - 1) * modpackItemsPerPage;

      const params = {
        query: modpackSearchQuery.value || undefined,
        gameVersions: selectedGameVersion.value ? [selectedGameVersion.value] : undefined,
        loaders: selectedLoader.value ? [selectedLoader.value] : undefined,
        categories: selectedCategory.value ? [selectedCategory.value] : undefined,
        limit: modpackItemsPerPage,
        offset: offset,
        sortBy: sortBy.value,
      };
      const result =
        sourceRef.value === 'curseforge'
          ? await api.curseforge.searchModpacks(params)
          : await api.modpack.searchModrinthModpacks(params);

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
