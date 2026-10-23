/**
 * 游戏启动流程
 * 唯一启动入口：登录态与 Java 兼容性前置校验，文件缺失可触发修复，最终交给 launch_instance
 */
import { ref } from 'vue';
import { useEventSubscription } from './useEventSubscription';
import { versionApi, instanceApi, javaApi } from '../services';
import { translate } from '../i18n';
import { useSettingsStore } from '../stores/settings';
import { useNotificationStore } from '../stores/notificationStore';
import { useAuthStore } from '../stores/authStore';
import { getErrorMessage } from '../utils/format';
import { logError } from '../utils/logger';
import type { DownloadProgress, JavaCompatibilityResult } from '../types/events';

interface JavaResolveResult {
  shouldLaunch: boolean;
  overrideJavaPath?: string;
  forceJavaMismatch?: boolean;
}

export function useGameLaunch() {
  const loading = ref(false);
  const isRepairing = ref(false);
  const repairProgress = ref<DownloadProgress | null>(null);
  const settingsStore = useSettingsStore();
  const notificationStore = useNotificationStore();
  const authStore = useAuthStore();

  const repairEventSub = useEventSubscription<DownloadProgress>('download-progress', (event) => {
    if (isRepairing.value) {
      repairProgress.value = event.payload;
    }
  });

  async function checkAndResolveJava(instanceName: string): Promise<JavaResolveResult> {
    let result: JavaCompatibilityResult;
    try {
      result = await javaApi.checkJavaCompatibility(instanceName);
    } catch {
      return { shouldLaunch: true };
    }

    // 能自动换到兼容 Java 的话交给后端换，不用打扰用户；换不到就必须让人确认
    if (result.compatible || (result.autoMatchEnabled && result.recommendedJavaPath)) {
      return { shouldLaunch: true };
    }

    const currentVer =
      result.currentJavaVersion != null ? `Java ${result.currentJavaVersion}` : translate('launch.javaUnknown');
    const requiredVer = `Java ${result.requiredJavaVersion}`;

    // 推荐项也是退而求其次时，把原因一起说清楚，用户才知道该不该去补一份
    const note = result.recommendedJavaWarning ? `\n\n${result.recommendedJavaWarning}` : '';

    const content = result.hardBlock
      ? translate('launch.javaHardBlock', { diagnostic: result.diagnostic })
      : translate('launch.javaMismatch', {
          current: currentVer,
          instance: instanceName,
          required: requiredVer,
          note,
        });

    const options: Array<{
      id: string;
      label: string;
      color?: string;
      variant?: 'elevated' | 'outlined' | 'text' | 'flat' | 'tonal' | 'plain';
    }> = [];

    if (result.recommendedJavaPath) {
      options.push({
        id: 'temp',
        label: translate('launch.useTempJava', { version: result.recommendedJavaVersion }),
        color: 'primary',
        variant: 'elevated',
      });
    }

    options.push({
      id: 'auto',
      label: translate('launch.enableAutoMatch'),
      color: 'success',
      variant: 'tonal',
    });

    options.push({
      id: 'manual',
      label: translate('launch.chooseManualJava'),
      color: 'warning',
      variant: 'outlined',
    });

    // 能走到这个弹窗就说明当前 Java 不满足判据，选“继续”必须把 force 传下去：
    // 否则后端会自己挑一份更合适的，用户的选择等于没生效
    options.push({
      id: 'continue',
      label:
        result.hardBlock || !result.recommendedJavaPath
          ? translate('launch.continueAnyway')
          : translate('launch.continueLaunch'),
      color: result.hardBlock ? 'error' : result.recommendedJavaPath ? 'info' : 'error',
      variant: 'text',
    });

    const selected = await notificationStore.choice(translate('launch.javaTitle'), content, options, 'warning');

    if (selected === 'continue') {
      return { shouldLaunch: true, forceJavaMismatch: true };
    }

    if (selected === 'temp' && result.recommendedJavaPath) {
      return { shouldLaunch: true, overrideJavaPath: result.recommendedJavaPath };
    }

    if (selected === 'auto') {
      settingsStore.autoMatchJava = true;
      await settingsStore.saveAutoMatchJava();
    } else if (selected === 'manual') {
      notificationStore.warning(translate('launch.manualJavaHint'));
    }

    return { shouldLaunch: false };
  }

  async function repairInstance(instanceName: string) {
    isRepairing.value = true;
    repairProgress.value = {
      bytes_downloaded: 0,
      total_bytes: 0,
      speed: 0,
      status: 'downloading',
      percent: 0,
    };

    await repairEventSub.subscribe();

    try {
      const mirror = settingsStore.downloadMirror || undefined;

      await versionApi.downloadVersion(instanceName, mirror);
      notificationStore.success(translate('launch.repairDone'), translate('launch.repairDoneHint'));
    } catch (err) {
      logError('Repair failed', err, 'useGameLaunch');
      notificationStore.error(translate('launch.repairFailed'), getErrorMessage(err), true);
    } finally {
      repairEventSub.unsubscribe();
      isRepairing.value = false;
      repairProgress.value = null;
    }
  }

  async function launchInstance(instanceName: string): Promise<boolean> {
    if (!instanceName) {
      notificationStore.warning(translate('launch.selectInstanceFirst'));
      return false;
    }

    if (!authStore.isLoggedIn) {
      notificationStore.warning(translate('launch.loginFirst'));
      return false;
    }

    try {
      loading.value = true;

      if (authStore.authType === 'microsoft' && authStore.msLoggedIn && authStore.isTokenExpired) {
        await authStore.tryRefreshMicrosoftToken();
      }

      const javaResolve = await checkAndResolveJava(instanceName);
      if (!javaResolve.shouldLaunch) {
        return false;
      }

      const missingFiles = await versionApi.validateVersionFiles(instanceName);
      if (missingFiles.length > 0) {
        const shouldRepair = await notificationStore.confirm(
          translate('launch.missingFilesTitle'),
          translate('launch.missingFilesContent', { count: missingFiles.length }, missingFiles.length),
          'warning',
        );

        if (shouldRepair) {
          await repairInstance(instanceName);
        }
        return false;
      }

      await instanceApi.launchInstance(instanceName, javaResolve.overrideJavaPath, javaResolve.forceJavaMismatch);
      // 命令返回只代表 Java 进程已派生；真正启动成功要等游戏窗口/日志，失败会走崩溃诊断
      notificationStore.info(translate('launch.starting'), instanceName);
      return true;
    } catch (err) {
      logError('Failed to launch instance', err, 'useGameLaunch');
      notificationStore.error(translate('launch.launchFailed'), getErrorMessage(err), true);
      return false;
    } finally {
      loading.value = false;
    }
  }

  /**
   * 崩溃诊断里换过 Java 之后的重启入口：跳过兼容性前置校验，
   * 用户刚在诊断弹窗里看过原因并做了选择，再弹一次是重复
   */
  async function relaunchWithJava(instanceName: string, javaPath: string): Promise<boolean> {
    try {
      loading.value = true;
      await instanceApi.launchInstance(instanceName, javaPath, false);
      notificationStore.info(
        translate('launch.relaunching'),
        translate('launch.relaunchedHint', { name: instanceName }),
      );
      return true;
    } catch (err) {
      logError('Failed to relaunch with another Java', err, 'useGameLaunch');
      notificationStore.error(translate('launch.launchFailed'), getErrorMessage(err), true);
      return false;
    } finally {
      loading.value = false;
    }
  }

  return {
    loading,
    isRepairing,
    repairProgress,
    launchInstance,
    relaunchWithJava,
  };
}
