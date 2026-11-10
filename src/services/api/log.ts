/** 启动记录、日志与崩溃报告 */
import { invokeCommand } from '../invoke';
import type { ContentTarget, CrashReportInfo, CrashReportText, LaunchRecord, LogTail } from '../../types/events';
import { dedupedInvoke } from './internal';

export const logApi = {
  async getHistory(instanceName: string): Promise<LaunchRecord[]> {
    return dedupedInvoke<LaunchRecord[]>('get_launch_history', { instanceName });
  },

  async getLatestLog(instanceName: string): Promise<LogTail> {
    return dedupedInvoke<LogTail>('get_latest_log', { instanceName });
  },

  async getLaunchLog(instanceName: string, launchId: number): Promise<LogTail> {
    return dedupedInvoke<LogTail>('get_launch_log', { instanceName, launchId });
  },

  async getCrashReports(instanceName: string): Promise<CrashReportInfo[]> {
    return dedupedInvoke<CrashReportInfo[]>('get_crash_reports', { instanceName });
  },

  async getCrashReport(instanceName: string, fileName: string): Promise<CrashReportText> {
    return dedupedInvoke<CrashReportText>('get_crash_report', { instanceName, fileName });
  },

  async openContent(instanceName: string, target: ContentTarget): Promise<void> {
    await invokeCommand('open_instance_content', { instanceName, target });
  },
};
