"""Take consistent, read-only SQLite snapshots before an application update."""
from datetime import datetime, timezone
from pathlib import Path
import sqlite3

root = Path(__file__).resolve().parents[1]
backup = root / 'data' / 'backups' / datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
backup.mkdir(parents=True, exist_ok=False)
for label, source in [('root-history', root / 'data/companion.db'),
                      ('backend-history', root / 'backend/data/companion.db')]:
    if not source.is_file():
        continue
    with sqlite3.connect(source.as_uri() + '?mode=ro', uri=True) as original:
        with sqlite3.connect(backup / f'{label}.db') as snapshot:
            original.backup(snapshot)
            result = snapshot.execute('PRAGMA integrity_check').fetchone()[0]
            if result != 'ok':
                raise RuntimeError(f'Backup integrity check failed for {label}')
    print(f'{label}: verified snapshot')
print(f'Backup directory: {backup}')
