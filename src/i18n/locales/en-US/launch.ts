/** Game launch, crash diagnostics and global error reporting */
export default {
  launch: {
    selectInstanceFirst: 'Select an instance first',
    loginFirst: 'Set a player name or sign in with Microsoft first',
    missingFilesTitle: 'Missing files',
    missingFilesContent:
      '{count} game file is missing. Repair it now? | {count} game files are missing. Repair them now?',
    repairDone: 'Repair complete',
    repairDoneHint: 'Please launch the game again',
    repairFailed: 'Repair failed',
    starting: 'Minecraft is starting',
    launchFailed: 'Launch failed',
    relaunching: 'Restarting Minecraft',
    relaunchedHint: '{name} is now using the selected Java',
    javaTitle: 'Java version notice',
    javaUnknown: 'unknown',
    javaHardBlock:
      'The current Java cannot run this instance. The backend reported:\n\n{diagnostic}\n\nHow would you like to proceed?',
    javaMismatch:
      'The current Java ({current}) does not meet the requirement of Minecraft {instance} (needs {required}).{note}\n\nHow would you like to proceed?',
    useTempJava: 'Use Java {version} for now',
    enableAutoMatch: 'Enable auto-match',
    chooseManualJava: 'Choose Java manually',
    fixCurrentJava: 'Always use Java {version} for this instance',
    javaFixedTitle: 'Java fixed',
    javaFixedHint: '{name} will keep using this Java and skip auto-match',
    javaFixedFailed: 'Failed to fix Java',
    continueAnyway: 'Launch with the current Java',
    continueLaunch: 'Continue launching',
    manualJavaHint: 'Pick a compatible Java path in Settings',
  },

  crash: {
    titleRecognized: 'Game crashed · a likely cause was identified',
    titleJavaHint: 'Game crashed · clues point to the Java version',
    titleUnknown: 'Game crashed · no known signature recognised',
    exitCode: 'Exit code {code}',
    roleRoot: 'Root cause',
    roleSymptom: 'Symptom',
    /** The culpritLabel itself comes from the backend; only the separator is ours */
    culpritLabel: '{label}:',
    environment: 'Environment',
    excerptToggle: 'Context around the match (lines {from}-{to})',
    rawToggle: 'Raw output ({count} lines)',
    copyReport: 'Copy diagnostic report',
    copySuccess: 'Diagnostic report copied',
    copySuccessHint: 'Conclusions, evidence lines and raw output are included, ready to paste upstream',
    copyFailed: 'Copy failed, please select the text in the dialog',
    close: 'Close',
    relaunchWithJava: 'Relaunch with Java {version}',
    relaunchWithJavaBelow: 'Retry with Java {version} anyway (below requirement)',
  },

  errors: {
    unhandledTitle: 'Unhandled UI error',
    commandTimeout: 'Backend command {command} did not respond within {seconds} seconds, please retry',
    commandFailed: 'Command failed: {command}',
    globalUnknown: 'Unknown error',
  },

  notifications: {
    copySuccess: 'Copied to clipboard',
    copyFailed: 'Copy failed, please select the text manually',
  },

  game: {
    exitedTitle: 'Minecraft exited',
    exitedHint: 'The game has exited',
    errorTitle: 'Minecraft runtime error',
  },

  diagnostics: {
    saveTitle: 'Export diagnostics',
    exported: 'Diagnostics exported',
    exportFailed: 'Failed to export diagnostics',
  },
};
