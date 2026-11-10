/** 存档列表、备份与导入 */
import { invokeCommand } from '../invoke';
import type { SaveBackupInfo, SaveImportSummary, SaveInfo } from '../../types/events';
import { invokeLong } from './internal';

export const saveApi = {
  async getSaves(instanceName: string): Promise<SaveInfo[]> {
    return invokeLong<SaveInfo[]>('get_instance_saves', { instanceName });
  },

  async getBackups(instanceName: string): Promise<SaveBackupInfo[]> {
    return invokeLong<SaveBackupInfo[]>('get_save_backups', { instanceName });
  },

  async backup(instanceName: string, world: string): Promise<SaveBackupInfo> {
    return invokeLong<SaveBackupInfo>('backup_save', { instanceName, world });
  },

  async restore(instanceName: string, world: string, fileName: string): Promise<void> {
    await invokeLong('restore_save_backup', { instanceName, world, fileName });
  },

  async deleteBackup(instanceName: string, world: string, fileName: string): Promise<void> {
    await invokeLong('delete_save_backup', { instanceName, world, fileName });
  },

  async importSaves(instanceName: string, sourcePaths: string[]): Promise<SaveImportSummary> {
    return invokeLong<SaveImportSummary>('import_saves', { instanceName, sourcePaths });
  },

  async openSavesFolder(instanceName: string): Promise<void> {
    await invokeCommand('open_saves_folder', { instanceName });
  },
};
