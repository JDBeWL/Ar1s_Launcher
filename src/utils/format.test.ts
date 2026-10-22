import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  formatTimeAgo,
  formatLastPlayed,
  formatFileSize,
  formatBytesProgress,
  formatBytesPerSecond,
  getLoaderIcon,
  getErrorMessage,
  getErrorCode,
} from './format';

describe('formatTimeAgo', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('刚刚（不到1分钟）', () => {
    const now = Date.now();
    expect(formatTimeAgo(now)).toBe('刚刚');
    expect(formatTimeAgo(now - 30_000)).toBe('刚刚');
  });

  it('N 分钟前', () => {
    const now = Date.now();
    expect(formatTimeAgo(now - 60_000)).toBe('1 分钟前');
    expect(formatTimeAgo(now - 30 * 60_000)).toBe('30 分钟前');
    expect(formatTimeAgo(now - 59 * 60_000)).toBe('59 分钟前');
  });

  it('N 小时前', () => {
    const now = Date.now();
    expect(formatTimeAgo(now - 3_600_000)).toBe('1 小时前');
    expect(formatTimeAgo(now - 12 * 3_600_000)).toBe('12 小时前');
    expect(formatTimeAgo(now - 23 * 3_600_000)).toBe('23 小时前');
  });

  it('N 天前', () => {
    const now = Date.now();
    expect(formatTimeAgo(now - 86_400_000)).toBe('1 天前');
    expect(formatTimeAgo(now - 6 * 86_400_000)).toBe('6 天前');
  });

  it('N 周前', () => {
    const now = Date.now();
    expect(formatTimeAgo(now - 7 * 86_400_000)).toBe('1 周前');
    expect(formatTimeAgo(now - 14 * 86_400_000)).toBe('2 周前');
    expect(formatTimeAgo(now - 29 * 86_400_000)).toBe('4 周前');
  });

  it('超过30天显示具体日期', () => {
    const now = Date.now();
    const result = formatTimeAgo(now - 31 * 86_400_000);
    expect(result).not.toContain('周前');
    expect(result).not.toContain('天前');
  });
});

describe('formatLastPlayed', () => {
  it('无时间戳返回"从未启动"', () => {
    expect(formatLastPlayed()).toBe('从未启动');
    expect(formatLastPlayed(undefined)).toBe('从未启动');
    expect(formatLastPlayed(0)).toBe('从未启动');
  });

  it('有时间戳时委托给 formatTimeAgo', () => {
    const now = Date.now();
    expect(formatLastPlayed(now)).toBe('刚刚');
    expect(formatLastPlayed(now - 3_600_000)).toBe('1 小时前');
  });
});

describe('formatFileSize', () => {
  it('小文件按 B 显示', () => {
    expect(formatFileSize(0)).toBe('0 KB');
    expect(formatFileSize(-1)).toBe('0 KB');
    expect(formatFileSize(512)).toBe('512 B');
  });

  it('不足 1 MB 按 KB 取整且不会显示 0 KB', () => {
    expect(formatFileSize(1024)).toBe('1 KB');
    expect(formatFileSize(200)).toBe('200 B');
    expect(formatFileSize(1500)).toBe('1 KB');
    expect(formatFileSize(1024 * 1024 - 1)).toBe('1024 KB');
  });

  it('MB 与 GB 保留小数', () => {
    expect(formatFileSize(1024 * 1024)).toBe('1.0 MB');
    expect(formatFileSize(12.5 * 1024 * 1024)).toBe('12.5 MB');
    expect(formatFileSize(3 * 1024 * 1024 * 1024)).toBe('3.00 GB');
  });

  it('非法数值兜底成 0 KB', () => {
    expect(formatFileSize(Number.NaN)).toBe('0 KB');
    expect(formatFileSize(Number.POSITIVE_INFINITY)).toBe('0 KB');
  });
});

describe('formatBytesProgress', () => {
  it('按量级给不同精度', () => {
    expect(formatBytesProgress(512)).toBe('512 B');
    expect(formatBytesProgress(1536)).toBe('1.5 KB');
    expect(formatBytesProgress(1024 * 1024)).toBe('1.00 MB');
    expect(formatBytesProgress(12.5 * 1024 * 1024)).toBe('12.50 MB');
  });

  it('和 formatFileSize 刻意不同：进度不取整', () => {
    expect(formatFileSize(1500)).toBe('1 KB');
    expect(formatBytesProgress(1500)).toBe('1.5 KB');
  });

  it('非法数值兜底成 0 B', () => {
    expect(formatBytesProgress(0)).toBe('0 B');
    expect(formatBytesProgress(-1)).toBe('0 B');
    expect(formatBytesProgress(Number.NaN)).toBe('0 B');
  });
});

describe('formatBytesPerSecond', () => {
  it('加 /s 后缀', () => {
    expect(formatBytesPerSecond(1024)).toBe('1.0 KB/s');
    expect(formatBytesPerSecond(2.5 * 1024 * 1024)).toBe('2.50 MB/s');
  });

  it('0 与非法值兜底成 0 B/s', () => {
    expect(formatBytesPerSecond(0)).toBe('0 B/s');
    expect(formatBytesPerSecond(Number.NaN)).toBe('0 B/s');
  });
});

describe('getLoaderIcon', () => {
  it('Forge 列表图标', () => {
    expect(getLoaderIcon('forge', 'list')).toBe('mdi-anvil');
    expect(getLoaderIcon('Forge', 'list')).toBe('mdi-anvil');
  });

  it('Fabric 列表图标', () => {
    expect(getLoaderIcon('fabric', 'list')).toBe('mdi-texture-box');
  });

  it('Quilt 列表图标', () => {
    expect(getLoaderIcon('quilt', 'list')).toBe('mdi-view-quilt');
  });

  it('NeoForge 列表图标', () => {
    expect(getLoaderIcon('neoforge', 'list')).toBe('mdi-anvil');
  });

  it('None / 原版列表图标', () => {
    expect(getLoaderIcon('None', 'list')).toBe('mdi-minecraft');
    expect(getLoaderIcon('none', 'list')).toBe('mdi-minecraft');
  });

  it('Fabric 选择图标', () => {
    expect(getLoaderIcon('fabric', 'select')).toBe('mdi-feather');
  });

  it('None 选择图标', () => {
    expect(getLoaderIcon('none', 'select')).toBe('mdi-close-circle-outline');
  });

  it('undefined 默认为 list 的 none', () => {
    expect(getLoaderIcon(undefined)).toBe('mdi-minecraft');
  });

  it('unknown 加载器用问号图标，不冒充原版', () => {
    expect(getLoaderIcon('unknown', 'list')).toBe('mdi-help-circle');
    expect(getLoaderIcon('unknown', 'select')).toBe('mdi-help-circle');
  });

  it('未登记在表里的加载器才走兜底图标', () => {
    expect(getLoaderIcon('sveleneku', 'list')).toBe('mdi-minecraft');
    expect(getLoaderIcon('sveleneku', 'select')).toBe('mdi-puzzle');
  });
});

describe('getErrorMessage', () => {
  it('Error 对象提取 message', () => {
    expect(getErrorMessage(new Error('test error'))).toBe('test error');
  });

  it('字符串直接返回', () => {
    expect(getErrorMessage('string error')).toBe('string error');
  });

  it('数字转为字符串', () => {
    expect(getErrorMessage(404)).toBe('404');
  });

  it('null 转为字符串', () => {
    expect(getErrorMessage(null)).toBe('null');
  });

  it('undefined 转为字符串', () => {
    expect(getErrorMessage(undefined)).toBe('undefined');
  });

  // Tauri 命令 reject 出来的就是这个形状：{ code, message }
  it('Tauri 错误载荷取 message', () => {
    expect(getErrorMessage({ code: 'custom', message: '整合包可能不完整' })).toBe('整合包可能不完整');
  });

  it('对象转为字符串', () => {
    expect(getErrorMessage({ code: 500 })).toBe('[object Object]');
  });
});

describe('getErrorCode', () => {
  it('读取后端稳定错误码', () => {
    expect(getErrorCode({ code: 'cancelled', message: '操作已取消' })).toBe('cancelled');
    expect(getErrorCode({ code: 'busy', message: '被占用' })).toBe('busy');
  });

  it('普通错误没有错误码', () => {
    expect(getErrorCode(new Error('test error'))).toBeNull();
    expect(getErrorCode('string error')).toBeNull();
    expect(getErrorCode(null)).toBeNull();
    expect(getErrorCode(undefined)).toBeNull();
  });
});
