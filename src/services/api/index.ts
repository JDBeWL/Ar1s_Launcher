/**
 * Tauri 后端接口调用模块
 *
 * 按域拆到同目录的各个文件，这里只负责聚合：具名导出供按需引用，`api` 供需要整表访问的调用方。
 * 公共的去重 / 缓存失效 / 长任务声明在 `./internal`。
 */
import { versionApi } from './version';
import { instanceApi } from './instance';
import { loaderApi } from './loader';
import { javaApi } from './java';
import { configApi } from './config';
import { saveApi } from './save';
import { logApi } from './log';
import { screenshotApi } from './screenshot';
import { userApi } from './user';
import { modpackApi } from './modpack';
import { modApi } from './mod';
import { curseforgeApi } from './curseforge';
import { resourcePackApi } from './resource-pack';
import { shaderPackApi } from './shader-pack';

export {
  versionApi,
  instanceApi,
  loaderApi,
  javaApi,
  configApi,
  saveApi,
  logApi,
  screenshotApi,
  userApi,
  modpackApi,
  modApi,
  curseforgeApi,
  resourcePackApi,
  shaderPackApi,
};

export const api = {
  version: versionApi,
  instance: instanceApi,
  loader: loaderApi,
  java: javaApi,
  config: configApi,
  save: saveApi,
  log: logApi,
  screenshot: screenshotApi,
  user: userApi,
  modpack: modpackApi,
  mod: modApi,
  curseforge: curseforgeApi,
  resourcePack: resourcePackApi,
  shaderPack: shaderPackApi,
};

export default api;
