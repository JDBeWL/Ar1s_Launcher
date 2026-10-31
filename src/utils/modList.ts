import type { ModInfo } from '../types/events';

/** 启停靠改扩展名，fileName 必须跟着变，否则再次开关会指向旧文件 */
export function setModEnabledInList(mods: ModInfo[], fileName: string, enabled: boolean): void {
  const target = mods.find((mod) => mod.fileName === fileName);
  if (!target) return;

  target.enabled = enabled;
  target.fileName = enabled ? fileName.replace(/\.disabled$/, '') : `${fileName}.disabled`;
}
