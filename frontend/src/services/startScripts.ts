/** How Companion is started again. The page cannot start the backend itself,
 *  so messages about a stopped runtime name the start script for each system,
 *  the one for this computer first. */
export type StartScript = { commands: readonly string[]; system: string };

export const START_SCRIPTS: readonly StartScript[] = [
  // run.bat for Command Prompt, where PowerShell's execution policy blocks run.ps1.
  { commands: ['.\\run.ps1', 'run.bat'], system: 'Windows' },
  { commands: ['./run.sh'], system: 'macOS and Linux' },
];

/** Both scripts, macOS and Linux first when `platform` names one of them
 *  (Chrome OS runs Companion in its Linux environment); otherwise (Windows, or
 *  a browser that does not say) Windows first. `platform` is what browsers
 *  report: "Windows", "macOS", "Linux", "Chrome OS", "Chromium OS" from
 *  userAgentData, or "Win32", "MacIntel", "Linux x86_64" from the older API. */
export function startScriptsFor(platform: string): readonly StartScript[] {
  return /mac|darwin|linux|x11|chrom(e|ium) ?os|cros/i.test(platform) ? [START_SCRIPTS[1], START_SCRIPTS[0]] : START_SCRIPTS;
}

/** The operating system this browser reports, or '' when it reports none. */
export function browserPlatform(): string {
  const nav = globalThis.navigator as (Navigator & { userAgentData?: { platform?: string } }) | undefined;
  return nav?.userAgentData?.platform || nav?.platform || '';
}
