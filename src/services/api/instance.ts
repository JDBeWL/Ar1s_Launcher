/** 实例列表与实例级操作 */
import { invokeCommand } from '../invoke';
import type { GameInstance, InstanceNameValidation, InstanceSettings, ModInfo } from '../../types/events';
import { dedupedInvoke, dedupedInvokeLong, invokeLong, withGameDirInfoInvalidation } from './internal';

export const instanceApi = {
  async getInstances(): Promise<GameInstance[]> {
    return dedupedInvokeLong<GameInstance[]>('get_instances');
  },

  async createInstance(
    newInstanceName: string,
    baseVersionId: string,
    loader?: { type: string; mc_version: string; loader_version: string },
  ): Promise<void> {
    return withGameDirInfoInvalidation(() => invokeLong('create_instance', { newInstanceName, baseVersionId, loader }));
  },

  async deleteInstance(instanceName: string): Promise<void> {
    return withGameDirInfoInvalidation(() => invokeLong('delete_instance', { instanceName }));
  },

  async renameInstance(oldName: string, newName: string): Promise<void> {
    return withGameDirInfoInvalidation(() => invokeLong('rename_instance', { oldName, newName }));
  },

  async openInstanceFolder(instanceName: string): Promise<void> {
    return invokeCommand('open_instance_folder', { instanceName });
  },

  /** mods 目录里的模组，含已禁用的；状态会变，不走缓存 */
  async getMods(instanceName: string): Promise<ModInfo[]> {
    return invokeLong<ModInfo[]>('get_instance_mods', { instanceName });
  },

  /** 启用/禁用模组（改名 *.jar <-> *.jar.disabled） */
  async setModEnabled(instanceName: string, fileName: string, enabled: boolean): Promise<void> {
    await invokeCommand('set_mod_enabled', { instanceName, fileName, enabled });
  },

  async deleteMod(instanceName: string, fileName: string): Promise<void> {
    await invokeCommand('delete_instance_mod', { instanceName, fileName });
  },

  /** 导出脱敏诊断包；目标路径由原生保存对话框选定 */
  async exportDiagnostics(instanceName: string, targetPath: string): Promise<void> {
    await invokeLong('export_diagnostic_bundle', { instanceName, targetPath });
  },

  async launchInstance(instanceName: string, overrideJavaPath?: string, forceJavaMismatch?: boolean): Promise<void> {
    await invokeLong('launch_instance', { instanceName, overrideJavaPath, forceJavaMismatch });
  },

  async getInstanceSettings(instanceName: string): Promise<InstanceSettings> {
    return dedupedInvoke<InstanceSettings>('get_instance_settings', { instanceName });
  },

  async setInstanceSettings(instanceName: string, settings: InstanceSettings): Promise<void> {
    await invokeCommand('set_instance_settings', { instanceName, settings });
  },

  async validateInstanceName(name: string): Promise<InstanceNameValidation> {
    return dedupedInvoke<InstanceNameValidation>('validate_instance_name_cmd', { name });
  },

  async checkInstanceNameAvailable(name: string): Promise<InstanceNameValidation> {
    return dedupedInvoke<InstanceNameValidation>('check_instance_name_available', { name });
  },
};
