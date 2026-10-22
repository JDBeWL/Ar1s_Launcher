/**
 * 格式化与排序工具
 * 相对时间、加载器图标与配色映射、错误载荷提取、MC 版本号比较与版本列表排序
 */
import { translate } from '../i18n';

export function formatTimeAgo(timestamp: number): string {
  const diff = Date.now() - timestamp;
  const minutes = Math.floor(diff / 60000);
  const hours = Math.floor(diff / 3600000);
  const days = Math.floor(diff / 86400000);

  if (minutes < 1) return translate('time.justNow');
  if (minutes < 60) return translate('time.minutesAgo', { count: minutes }, minutes);
  if (hours < 24) return translate('time.hoursAgo', { count: hours }, hours);
  if (days < 7) return translate('time.daysAgo', { count: days }, days);
  if (days < 30) {
    const weeks = Math.floor(days / 7);
    return translate('time.weeksAgo', { count: weeks }, weeks);
  }
  return new Date(timestamp).toLocaleDateString();
}

export function formatLastPlayed(timestamp?: number): string {
  if (!timestamp) return translate('time.neverLaunched');
  return formatTimeAgo(timestamp);
}

/** 资源包、模组这类文件体积展示：不到 1 MB 按 KB 取整，避免出现「0 MB」 */
export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 KB';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/**
 * 下载进度里的体积展示：保留两位有效小数并随量级调整精度。[`formatFileSize`] 会把 1500 B
 * 取整成「1 KB」，而进度条每帧都在变，取整会让数字长时间不动、看着像卡住
 */
export function formatBytesProgress(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';

  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  const digits = index === 0 ? 0 : index >= 2 ? 2 : 1;
  return `${value.toFixed(digits)} ${units[index]}`;
}

/** 下载速度：按 KB/s、MB/s 展示，0 或非法值时给 `0 B/s` */
export function formatBytesPerSecond(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return '0 B/s';
  return `${formatBytesProgress(bytesPerSecond)}/s`;
}

const LOADER_ICONS: Record<string, { list: string; select: string }> = {
  forge: { list: 'mdi-anvil', select: 'mdi-anvil' },
  fabric: { list: 'mdi-texture-box', select: 'mdi-feather' },
  quilt: { list: 'mdi-view-quilt', select: 'mdi-view-quilt' },
  neoforge: { list: 'mdi-anvil', select: 'mdi-anvil' },
  modded: { list: 'mdi-puzzle', select: 'mdi-puzzle' },
  unknown: { list: 'mdi-help-circle', select: 'mdi-help-circle' },
  none: { list: 'mdi-minecraft', select: 'mdi-close-circle-outline' },
};

const LOADER_COLORS: Record<string, { color: string; bgColor: string }> = {
  forge: { color: 'blue', bgColor: 'blue-container' },
  fabric: { color: 'red', bgColor: 'red-container' },
  quilt: { color: 'purple', bgColor: 'purple-container' },
  neoforge: { color: 'orange', bgColor: 'orange-container' },
  modded: { color: 'teal', bgColor: 'teal-container' },
  unknown: { color: 'grey', bgColor: 'grey-container' },
  none: { color: 'primary', bgColor: 'primary-container' },
};

export function getLoaderColor(loaderType?: string): { color: string; bgColor: string } {
  const key = (loaderType || 'none').toLowerCase();
  return LOADER_COLORS[key] || LOADER_COLORS['none'];
}

export function getLoaderIcon(loaderType?: string, context: 'list' | 'select' = 'list'): string {
  const key = (loaderType || 'none').toLowerCase();
  const icons = LOADER_ICONS[key];

  if (icons) {
    return context === 'select' ? icons.select : icons.list;
  }

  return context === 'select' ? 'mdi-puzzle' : 'mdi-minecraft';
}

/** Tauri 命令 reject 的载荷是 { message } 对象，不是 Error */
export function getErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

/** 后端 LauncherError 的稳定错误码；普通 JS Error 没有 code */
export function getErrorCode(error: unknown): string | null {
  if (error && typeof error === 'object' && 'code' in error) {
    return String((error as { code: unknown }).code);
  }
  return null;
}

interface ParsedVersion {
  type: 'release' | 'rc' | 'pre' | 'snapshot' | 'other' | 'unknown';
  parts: number[];
  suffixNum: number;
}

const VERSION_TYPE_PRIORITY: Record<string, number> = {
  release: 3,
  rc: 2,
  pre: 1,
  other: 0,
  snapshot: -1,
  unknown: -2,
};

function parseVersion(v: string): ParsedVersion {
  const snapshotMatch = v.match(/^(\d+)w(\d+)([a-z])$/);
  if (snapshotMatch) {
    return { type: 'snapshot', parts: [parseInt(snapshotMatch[1]), parseInt(snapshotMatch[2])], suffixNum: 0 };
  }

  const match = v.match(/^([\d.]+)(?:-(.+))?$/);
  if (!match) return { type: 'unknown', parts: [0], suffixNum: 0 };

  const parts = match[1].split('.').map((n) => parseInt(n) || 0);
  const suffix = match[2] || '';

  if (suffix.startsWith('rc')) {
    return { type: 'rc', parts, suffixNum: parseInt(suffix.slice(2)) || 0 };
  }
  if (suffix.startsWith('pre')) {
    return { type: 'pre', parts, suffixNum: parseInt(suffix.slice(3)) || 0 };
  }
  if (suffix) {
    return { type: 'other', parts, suffixNum: 0 };
  }

  return { type: 'release', parts, suffixNum: 0 };
}

const parseVersionCache = new Map<string, ParsedVersion>();

function cachedParseVersion(v: string): ParsedVersion {
  let parsed = parseVersionCache.get(v);
  if (!parsed) {
    parsed = parseVersion(v);
    parseVersionCache.set(v, parsed);
  }
  return parsed;
}

export function compareVersionDesc(a: string, b: string): number {
  const va = cachedParseVersion(a);
  const vb = cachedParseVersion(b);

  if (va.type === 'snapshot' && vb.type !== 'snapshot') return 1;
  if (vb.type === 'snapshot' && va.type !== 'snapshot') return -1;

  const maxLen = Math.max(va.parts.length, vb.parts.length);
  for (let i = 0; i < maxLen; i++) {
    const av = va.parts[i] ?? 0;
    const bv = vb.parts[i] ?? 0;
    if (av !== bv) return bv - av;
  }

  const typeDiff = (VERSION_TYPE_PRIORITY[vb.type] ?? 0) - (VERSION_TYPE_PRIORITY[va.type] ?? 0);
  if (typeDiff !== 0) return typeDiff;

  return (vb.suffixNum ?? 0) - (va.suffixNum ?? 0);
}

export type SortOrder = 'newest' | 'oldest';

export interface VersionLike {
  id?: string;
  version?: string;
  releaseTime?: string;
}

function getReleaseTime(item: VersionLike): number {
  if (!item.releaseTime) return 0;
  return new Date(item.releaseTime).getTime() || 0;
}

export function sortVersionsByReleaseTime<T extends VersionLike>(items: T[], order: SortOrder = 'newest'): T[] {
  return [...items].sort((a, b) => {
    const timeA = getReleaseTime(a);
    const timeB = getReleaseTime(b);
    return order === 'newest' ? timeB - timeA : timeA - timeB;
  });
}
