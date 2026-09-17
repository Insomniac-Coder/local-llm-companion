import { Fragment } from 'react';
import { browserPlatform, startScriptsFor } from '../services/startScripts';

/** "`.\run.ps1` or `run.bat` on Windows, or `./run.sh` on macOS and Linux", this computer's scripts first. */
export default function StartScripts() {
  return (
    <>
      {startScriptsFor(browserPlatform()).map(({ commands, system }, index) => (
        <Fragment key={system}>
          {index > 0 && ', or '}
          {commands.map((command, position) => (
            <Fragment key={command}>
              {position > 0 && ' or '}
              <code>{command}</code>
            </Fragment>
          ))}
          {' on '}{system}
        </Fragment>
      ))}
    </>
  );
}
