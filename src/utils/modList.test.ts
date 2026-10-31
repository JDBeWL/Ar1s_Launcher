import { describe, expect, it } from 'vitest';
import type { ModInfo } from '../types/events';
import { setModEnabledInList } from './modList';

function mod(fileName: string, enabled: boolean): ModInfo {
  return { fileName, displayName: fileName, enabled, size: 1024 };
}

describe('setModEnabledInList', () => {
  it('禁用后只改目标行，并把文件名换成 .disabled', () => {
    const other = mod('other.jar', false);
    const mods = [mod('target.jar', true), other];

    setModEnabledInList(mods, 'target.jar', false);

    expect(mods[0]).toMatchObject({ enabled: false, fileName: 'target.jar.disabled' });
    expect(mods[1]).toBe(other);
  });

  it('启用时去掉 .disabled 后缀', () => {
    const mods = [mod('target.jar.disabled', false)];

    setModEnabledInList(mods, 'target.jar.disabled', true);

    expect(mods[0]).toMatchObject({ enabled: true, fileName: 'target.jar' });
  });

  it('找不到目标模组时列表保持不变', () => {
    const mods = [mod('target.jar', true)];

    setModEnabledInList(mods, 'missing.jar', false);

    expect(mods).toHaveLength(1);
    expect(mods[0].enabled).toBe(true);
  });
});
