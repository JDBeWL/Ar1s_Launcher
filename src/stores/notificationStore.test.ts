/**
 * 对话框 resolver 槽位测试
 * confirm / choice 各只有一个 resolver 槽位，验证重复调用时前一个 await 不会永久挂起
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useNotificationStore } from './notificationStore';

describe('notificationStore 对话框', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('第二次 confirm 先把前一个 await 按 false 结算', async () => {
    const store = useNotificationStore();
    const first = store.confirm('t1', 'c1');
    const second = store.confirm('t2', 'c2');

    store.handleConfirm(true);

    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(true);
  });

  it('第二次 choice 先把前一个 await 按 null 结算', async () => {
    const store = useNotificationStore();
    const options = [{ id: 'go', label: '继续' }];
    const first = store.choice('t1', 'c1', options);
    const second = store.choice('t2', 'c2', options);

    store.handleChoice('go');

    await expect(first).resolves.toBeNull();
    await expect(second).resolves.toBe('go');
  });
});

describe('notificationStore 定时器', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('到时自动消失', () => {
    const store = useNotificationStore();
    store.notify('info', 't', 'm', 1000);

    expect(store.notifications).toHaveLength(1);
    vi.advanceTimersByTime(1001);
    expect(store.notifications).toHaveLength(0);
  });

  it('手动关闭后定时器不再触发', () => {
    const store = useNotificationStore();
    const id = store.notify('info', 't', 'm', 1000);

    store.removeNotification(id);
    expect(store.notifications).toHaveLength(0);

    // 定时器已清掉：推进时间不应再改动通知列表
    vi.advanceTimersByTime(2000);
    expect(store.notifications).toHaveLength(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('超出上限被挤掉的通知也会清掉定时器', () => {
    const store = useNotificationStore();
    // MAX_NOTIFICATIONS 是 5，塞 6 条会挤掉最早一条
    for (let i = 0; i < 6; i += 1) {
      store.notify('info', `t${i}`, 'm', 60_000);
    }

    expect(store.notifications).toHaveLength(5);
    expect(vi.getTimerCount()).toBe(5);
  });
});
