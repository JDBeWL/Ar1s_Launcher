/** UI text for backend error codes: keys must match `LauncherError::localized` codes in Rust */
export default {
  errors: {
    backend: {
      // Coarse wrappers: params[0] is the underlying reason
      io: 'File operation failed: {0}',
      http: 'Network request failed: {0}',
      json: 'Could not parse the response data: {0}',
      zip: 'Could not process the archive: {0}',
      tauri: 'Could not reach the launcher backend: {0}',
      cancelled: 'The operation was cancelled',
      busy: 'Another launcher instance is using {0}',
      auth_denied: 'Sign-in was cancelled',
      auth_expired: 'Your sign-in has expired, please sign in again',
      // Business errors without a dedicated code keep the original text
      custom: 'Operation failed: {0}',
      // Specific business errors
      disk_insufficient: 'Not enough disk space: about {0} required, {1} available ({2})',
      java_not_found: 'No usable Java found. Pick or download a Java runtime in Settings → Java.',
      java_path_invalid: 'The Java path in Settings is invalid: {0} (resolved to {1})',
      version_not_found: 'Version {0} was not found. Refresh the version list and try again.',
      instance_exists: 'Instance "{0}" already exists. Pick another name.',
      instance_locked: 'Instance "{0}" is being deleted or renamed. Try again in a moment.',
      main_jar_missing: 'The main game jar is missing: {0}. Run "Repair instance" before launching.',
      modpack_unreadable: 'Could not read the modpack (it may not be a zip archive): {0}',
      modpack_missing_index: 'The modpack has no {0}, so it is probably not a Modrinth pack.',
      modpack_index_too_large: '{0} is larger than the {1} byte limit and was rejected.',
      modpack_index_invalid: 'Could not parse {0}: {1}',
      mod_file_not_found: 'The selected mod file was not found.',
      mod_no_compatible_file: 'This mod has no file for the instance game version and loader.',
      instance_version_unknown: 'Could not determine the instance Minecraft version.',
    },
  },
};
