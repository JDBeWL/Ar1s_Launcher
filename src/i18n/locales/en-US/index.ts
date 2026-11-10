/** English messages: split by namespace, merged into one tree here */
import { en } from 'vuetify/locale';
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
import logs from './logs';
import screenshots from './screenshots';
import settings from './settings';
import addInstance from './addInstance';
import launch from './launch';
import backendErrors from './backendErrors';

// Vuetify looks its built-in strings up under the `$vuetify.` prefix, so its catalog
// must be nested there; flattening it to the root leaves pagination, empty selects and
// date pickers showing raw keys
export default mergeMessages(
  { $vuetify: en },
  common,
  shell,
  home,
  download,
  instance,
  mods,
  resourcePacks,
  shaderPacks,
  saves,
  logs,
  screenshots,
  settings,
  addInstance,
  launch,
  backendErrors,
);
