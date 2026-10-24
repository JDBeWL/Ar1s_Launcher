/** 简体中文消息：按命名空间拆分，这里聚合成一棵树 */
import { zhHans } from 'vuetify/locale';
import { mergeMessages } from '../../merge';
import common from './common';
import shell from './shell';
import home from './home';
import download from './download';
import instance from './instance';
import mods from './mods';
import resourcePacks from './resourcePacks';
import shaderPacks from './shaderPacks';
import saves from './saves';
import settings from './settings';
import addInstance from './addInstance';
import launch from './launch';

// Vuetify 组件查文案时用的是 `$vuetify.xxx` 前缀，所以它的语言包必须挂在 `$vuetify` 下，
// 不能平铺到根上，否则分页、选择框空状态、日期面板会退回英文 key
export default mergeMessages(
  { $vuetify: zhHans },
  common,
  shell,
  home,
  download,
  instance,
  mods,
  resourcePacks,
  shaderPacks,
  saves,
  settings,
  addInstance,
  launch,
);
