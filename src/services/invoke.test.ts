import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invokeMock, warnMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  warnMock: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('../utils/logger', () => ({ logWarn: warnMock }));

import { DEFAULT_COMMAND_TIMEOUT_MS, LauncherCommandError, NO_TIMEOUT, invokeCommand } from './invoke';
import { DEFAULT_LOCALE, setLocale } from '../i18n';

describe('invokeCommand', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    warnMock.mockReset();
    vi.useRealTimers();
  });

  it('成功时透传结果', async () => {
    invokeMock.mockResolvedValue({ ok: true });

    await expect(invokeCommand('get_versions')).resolves.toEqual({ ok: true });
    expect(invokeMock).toHaveBeenCalledWith('get_versions', undefined);
  });

  it('保留后端稳定错误码与命令名', async () => {
    invokeMock.mockRejectedValue({ code: 'busy', message: '另一个启动器实例正在占用下载锁' });

    const error = await invokeCommand('download_version').catch((value: unknown) => value);

    expect(error).toBeInstanceOf(LauncherCommandError);
    expect(error).toMatchObject({
      command: 'download_version',
      code: 'busy',
      message: '另一个启动器实例正在占用下载锁',
    });
  });

  it('字符串 reject 归为 ipc', async () => {
    invokeMock.mockRejectedValue('invalid args `instanceName` for command');

    await expect(invokeCommand('delete_instance')).rejects.toMatchObject({
      command: 'delete_instance',
      code: 'ipc',
      message: 'invalid args `instanceName` for command',
    });
  });

  it('普通 Error 归为 ipc 并保留消息', async () => {
    invokeMock.mockRejectedValue(new Error('后端未响应'));

    await expect(invokeCommand('get_instances')).rejects.toMatchObject({
      code: 'ipc',
      message: '后端未响应',
    });
  });

  it('cancelled 错误可直接识别', async () => {
    invokeMock.mockRejectedValue({ code: 'cancelled', message: '操作已取消' });

    const error = (await invokeCommand('install_modrinth_modpack').catch(
      (value: unknown) => value,
    )) as LauncherCommandError;

    expect(error.cancelled).toBe(true);
  });

  it('无 message 的对象不会显示成 [object Object]', async () => {
    invokeMock.mockRejectedValue({ code: 'custom', detail: 'x' });

    const error = (await invokeCommand('get_instances').catch((value: unknown) => value)) as LauncherCommandError;

    expect(error.message).not.toBe('[object Object]');
    expect(error.message).toContain('detail');
  });

  it('默认超时后拒绝并标记 timeout', async () => {
    vi.useFakeTimers();
    invokeMock.mockReturnValue(new Promise(() => {}));

    const promise = invokeCommand('get_versions');
    const assertion = expect(promise).rejects.toMatchObject({
      command: 'get_versions',
      code: 'timeout',
    });

    await vi.advanceTimersByTimeAsync(DEFAULT_COMMAND_TIMEOUT_MS);
    await assertion;
  });

  it('长任务在调用点声明不设超时后才不会被打断', async () => {
    vi.useFakeTimers();
    let resolveRequest: ((value: string) => void) | undefined;
    invokeMock.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveRequest = resolve;
      }),
    );

    let settled = false;
    const promise = invokeCommand('download_version', undefined, NO_TIMEOUT).finally(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(DEFAULT_COMMAND_TIMEOUT_MS * 5);
    expect(settled).toBe(false);

    resolveRequest?.('done');
    await expect(promise).resolves.toBe('done');
  });

  it('没有声明不超时的长任务仍受默认超时保护', async () => {
    // 这条防止「命令名白名单」被悄悄加回来：超时策略只认调用点，不认命令名
    vi.useFakeTimers();
    invokeMock.mockReturnValue(new Promise(() => {}));

    const assertion = expect(invokeCommand('download_version')).rejects.toMatchObject({
      code: 'timeout',
    });

    await vi.advanceTimersByTimeAsync(DEFAULT_COMMAND_TIMEOUT_MS);
    await assertion;
  });

  it('可显式关闭单次调用的超时', async () => {
    vi.useFakeTimers();
    let resolveRequest: ((value: string) => void) | undefined;
    invokeMock.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveRequest = resolve;
      }),
    );

    let settled = false;
    const promise = invokeCommand('get_versions', undefined, { timeoutMs: 0 }).finally(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(DEFAULT_COMMAND_TIMEOUT_MS * 2);
    expect(settled).toBe(false);

    resolveRequest?.('done');
    await expect(promise).resolves.toBe('done');
  });
});

describe('后端错误码本地化', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.useRealTimers();
    setLocale(DEFAULT_LOCALE);
  });

  it('英文界面把错误码翻成英文并填充位置参数', async () => {
    setLocale('en-US');
    invokeMock.mockRejectedValue({
      code: 'disk_insufficient',
      message: '磁盘空间不足：需要约 2.0 GB，当前可用 1.0 GB（D:/game）',
      params: ['2.0 GB', '1.0 GB', 'D:/game'],
    });

    const error = (await invokeCommand('download_version').catch((value: unknown) => value)) as LauncherCommandError;

    expect(error.code).toBe('disk_insufficient');
    expect(error.message).toBe('Not enough disk space: about 2.0 GB required, 1.0 GB available (D:/game)');
    expect(error.params).toEqual(['2.0 GB', '1.0 GB', 'D:/game']);
  });

  it('中文界面用中文文案', async () => {
    invokeMock.mockRejectedValue({
      code: 'java_not_found',
      message: '未在配置中找到有效的Java路径，且系统PATH中也未找到Java。',
      params: [],
    });

    const error = (await invokeCommand('launch_instance').catch((value: unknown) => value)) as LauncherCommandError;

    expect(error.message).toContain('没有找到可用的 Java');
  });

  it('粗粒度 io 错误用本地化外壳包住底层原因', async () => {
    invokeMock.mockRejectedValue({
      code: 'io',
      message: 'IO 错误: 拒绝访问 (os error 5)',
      params: ['拒绝访问 (os error 5)'],
    });

    const error = (await invokeCommand('get_instances').catch((value: unknown) => value)) as LauncherCommandError;

    expect(error.message).toBe('文件操作失败：拒绝访问 (os error 5)');
  });

  it('没有细分码的 custom 错误用本地化外壳保留原文', () => {
    setLocale('en-US');
    invokeMock.mockRejectedValue({ code: 'custom', message: '实例目录里缺少 client.jar' });

    return expect(invokeCommand('create_instance')).rejects.toMatchObject({
      message: 'Operation failed: 实例目录里缺少 client.jar',
    });
  });

  it('参数不足以填满模板时回退到后端原文', async () => {
    // 旧后端只发 code 和 message，没带 params：不能把 {0} 露给用户
    invokeMock.mockRejectedValue({ code: 'disk_insufficient', message: '磁盘空间不足' });

    const error = (await invokeCommand('download_version').catch((value: unknown) => value)) as LauncherCommandError;

    expect(error.message).toBe('磁盘空间不足');
  });

  it('未知错误码回退到原文，不显示原始 key', async () => {
    invokeMock.mockRejectedValue({
      code: 'not_registered_yet',
      message: '后端新加的错误',
      params: [],
    });

    const error = (await invokeCommand('something_new').catch((value: unknown) => value)) as LauncherCommandError;

    expect(error.message).toBe('后端新加的错误');
  });
});
