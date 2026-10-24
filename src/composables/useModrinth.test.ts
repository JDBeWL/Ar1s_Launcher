import { beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';

vi.mock('../services', () => ({
  api: {
    modpack: { searchModrinthModpacks: vi.fn() },
    curseforge: { searchModpacks: vi.fn(), getModpackCategories: vi.fn() },
    version: { getVersions: vi.fn() },
  },
}));
vi.mock('../i18n', () => ({ translate: (key: string) => key }));

import { api } from '../services';
import { useModrinth } from './useModrinth';

const search = vi.mocked(api.modpack.searchModrinthModpacks);

describe('useModrinth 搜索提交', () => {
  beforeEach(() => {
    search.mockReset();
    search.mockResolvedValue({ hits: [], total_hits: 0 } as never);
  });

  it('输入搜索词不会自动发请求', async () => {
    const { modpackSearchQuery } = useModrinth();

    modpackSearchQuery.value = 'sodium';
    await nextTick();
    modpackSearchQuery.value = 'sodium 1.20';
    await nextTick();

    expect(search).not.toHaveBeenCalled();
  });

  it('回车提交时才搜索，并回到第一页', async () => {
    const { modpackSearchQuery, modpackCurrentPage, submitModpackSearch } = useModrinth();
    modpackSearchQuery.value = 'sodium';
    modpackCurrentPage.value = 3;

    await submitModpackSearch();

    expect(search).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledWith(expect.objectContaining({ query: 'sodium', offset: 0 }));
  });

  it('清空搜索词会恢复完整列表', async () => {
    const { modpackSearchQuery } = useModrinth();
    modpackSearchQuery.value = 'sodium';
    await nextTick();

    modpackSearchQuery.value = '';
    await nextTick();

    expect(search).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledWith(expect.objectContaining({ query: undefined, offset: 0 }));
  });
});
