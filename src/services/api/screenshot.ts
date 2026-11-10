/** 游戏截图 */
import { invokeCommand } from '../invoke';
import type { ScreenshotInfo } from '../../types/events';

export const screenshotApi = {
  async getScreenshots(instanceName: string): Promise<ScreenshotInfo[]> {
    return invokeCommand<ScreenshotInfo[]>('get_instance_screenshots', { instanceName });
  },

  async copyScreenshot(instanceName: string, fileName: string): Promise<void> {
    await invokeCommand('copy_instance_screenshot', { instanceName, fileName });
  },

  async openFolder(instanceName: string): Promise<void> {
    await invokeCommand('open_screenshots_folder', { instanceName });
  },
};
