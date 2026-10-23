/**
 * 通知与对话框存储
 * 维护最多 5 条自动消失的 snackbar，另提供返回 Promise 的 confirm 与 choice
 */
import { defineStore } from 'pinia';
import { ref } from 'vue';

export type NotificationType = 'success' | 'error' | 'warning' | 'info';

export interface Notification {
  id: number;
  type: NotificationType;
  title: string;
  message?: string;
  timeout?: number;
  showDetail?: boolean;
}

export interface ChoiceOption {
  id: string;
  label: string;
  description?: string;
  color?: string;
  variant?: 'elevated' | 'outlined' | 'text' | 'flat' | 'tonal' | 'plain';
}

let notificationId = 0;
const MAX_NOTIFICATIONS = 5;

export const useNotificationStore = defineStore('notification', () => {
  const notifications = ref<Notification[]>([]);

  /** 每条通知的自动消失定时器；手动关闭或超出上限时要把它清掉，否则留一个空转的定时器 */
  const timers = new Map<number, ReturnType<typeof setTimeout>>();

  const dialogVisible = ref(false);
  const dialogTitle = ref('');
  const dialogContent = ref('');
  const dialogType = ref<NotificationType>('info');

  const confirmVisible = ref(false);
  const confirmTitle = ref('');
  const confirmContent = ref('');
  const confirmType = ref<NotificationType>('warning');
  let confirmResolve: ((value: boolean) => void) | null = null;

  const choiceVisible = ref(false);
  const choiceTitle = ref('');
  const choiceContent = ref('');
  const choiceType = ref<NotificationType>('warning');
  const choiceOptions = ref<ChoiceOption[]>([]);
  let choiceResolve: ((value: string | null) => void) | null = null;

  function notify(type: NotificationType, title: string, message?: string, timeout = 4000) {
    const id = ++notificationId;
    notifications.value.push({ id, type, title, message, timeout });

    if (notifications.value.length > MAX_NOTIFICATIONS) {
      const dropped = notifications.value.splice(0, notifications.value.length - MAX_NOTIFICATIONS);
      for (const old of dropped) {
        clearTimer(old.id);
      }
    }

    if (timeout > 0) {
      const timer = setTimeout(() => {
        timers.delete(id);
        removeNotification(id);
      }, timeout);
      timers.set(id, timer);
    }

    return id;
  }

  function success(title: string, message?: string) {
    return notify('success', title, message);
  }

  function error(title: string, message?: string, showDialog = false) {
    if (showDialog && message) {
      // 走详情对话框时不再叠加 snackbar
      showErrorDialog(title, message);
      return -1;
    }
    // 6000 要和 GlobalNotification.vue 的 snackbarTimeout 保持一致
    return notify('error', title, message, 6000);
  }

  function warning(title: string, message?: string) {
    return notify('warning', title, message);
  }

  function info(title: string, message?: string) {
    return notify('info', title, message);
  }

  function clearTimer(id: number) {
    const timer = timers.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      timers.delete(id);
    }
  }

  function removeNotification(id: number) {
    clearTimer(id);
    const index = notifications.value.findIndex((n) => n.id === id);
    if (index > -1) {
      notifications.value.splice(index, 1);
    }
  }

  function showErrorDialog(title: string, content: string) {
    dialogTitle.value = title;
    dialogContent.value = content;
    dialogType.value = 'error';
    dialogVisible.value = true;
  }

  function showInfoDialog(title: string, content: string) {
    dialogTitle.value = title;
    dialogContent.value = content;
    dialogType.value = 'info';
    dialogVisible.value = true;
  }

  function closeDialog() {
    dialogVisible.value = false;
  }

  function confirm(title: string, content: string, type: NotificationType = 'warning'): Promise<boolean> {
    // resolver 只有一个槽位，前一个没答的必须先按"取消"结算，否则它的 await 永远挂着
    confirmResolve?.(false);

    confirmTitle.value = title;
    confirmContent.value = content;
    confirmType.value = type;
    confirmVisible.value = true;

    return new Promise((resolve) => {
      confirmResolve = resolve;
    });
  }

  function handleConfirm(result: boolean) {
    confirmVisible.value = false;
    if (confirmResolve) {
      confirmResolve(result);
      confirmResolve = null;
    }
  }

  function choice(
    title: string,
    content: string,
    options: ChoiceOption[],
    type: NotificationType = 'warning',
  ): Promise<string | null> {
    choiceResolve?.(null);

    choiceTitle.value = title;
    choiceContent.value = content;
    choiceType.value = type;
    choiceOptions.value = options;
    choiceVisible.value = true;

    return new Promise((resolve) => {
      choiceResolve = resolve;
    });
  }

  function handleChoice(result: string | null) {
    choiceVisible.value = false;
    if (choiceResolve) {
      choiceResolve(result);
      choiceResolve = null;
    }
  }

  return {
    notifications,
    dialogVisible,
    dialogTitle,
    dialogContent,
    dialogType,
    confirmVisible,
    confirmTitle,
    confirmContent,
    confirmType,
    notify,
    success,
    error,
    warning,
    info,
    removeNotification,
    showErrorDialog,
    showInfoDialog,
    closeDialog,
    confirm,
    handleConfirm,
    choiceVisible,
    choiceTitle,
    choiceContent,
    choiceType,
    choiceOptions,
    choice,
    handleChoice,
  };
});
