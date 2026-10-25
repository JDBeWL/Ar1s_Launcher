/**
 * 诊断包导出
 * 用原生保存对话框选目标；后端只收日志、崩溃报告、版本 JSON 和模组文件名
 */
import { ref } from 'vue';
import { save } from '@tauri-apps/plugin-dialog';
import { instanceApi } from '../services';
import { translate } from '../i18n';
import { useNotificationStore } from '../stores/notificationStore';
import { getErrorMessage } from '../utils/format';
import { logError } from '../utils/logger';
import type { GameInstance } from '../types/events';

type InstanceRef = Pick<GameInstance, 'name'>;

function defaultFileName(instanceName: string): string {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, '0');
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const safeName = instanceName.replace(/[\\/:*?"<>|]/g, '_');
  return `ar1s-diagnostics-${safeName}-${stamp}.zip`;
}

export function useDiagnosticsExport() {
  const notificationStore = useNotificationStore();
  const exportingInstance = ref<string | null>(null);

  async function exportDiagnostics(instance: InstanceRef): Promise<void> {
    if (exportingInstance.value) return;
    exportingInstance.value = instance.name;

    try {
      const selected = await save({
        title: translate('diagnostics.saveTitle'),
        defaultPath: defaultFileName(instance.name),
        filters: [{ name: 'Zip', extensions: ['zip'] }],
      });
      if (!selected) return;

      const target = selected.toLowerCase().endsWith('.zip') ? selected : `${selected}.zip`;
      await instanceApi.exportDiagnostics(instance.name, target);
      notificationStore.success(translate('diagnostics.exported'), target.split(/[\\/]/).pop() || target);
    } catch (error) {
      logError('Failed to export diagnostics', error, 'useDiagnosticsExport');
      notificationStore.error(translate('diagnostics.exportFailed'), getErrorMessage(error));
    } finally {
      exportingInstance.value = null;
    }
  }

  return { exportingInstance, exportDiagnostics };
}
