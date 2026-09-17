"""Download a GGUF model into its own folder under models/, verify it, and check
that the bundled llama.cpp runtime can actually load it.

    python modeldownloader.py hf <owner>/<repository> <file>.gguf
    python modeldownloader.py hf <owner>/<repository> <file>.gguf --mmproj <projector>.gguf
    python modeldownloader.py ollama <model>:<tag>

Every download:
  * goes to models/<folder>/, never loose in models/ (--name overrides the folder);
  * is written to <file>.part and resumed from there if interrupted;
  * is checked against the size and SHA-256 the source publishes before it is
    renamed into place: an unverified download is indistinguishable from a
    truncated one until it fails to load;
  * is inspected (architecture, tensors, chat template) and probed with the
    runtime's own loader, which reports exactly what it cannot read.

About Ollama registry models. Ollama stores models in its own packaging: a
multimodal model's vision and audio towers inside the same file as the
language model, tokenizer and hyperparameter metadata that differ from
upstream conversions, and the chat template in a separate Go-template layer.
Ollama's patched llama.cpp repairs those files in memory when it loads them,
which is why a blob that works in Ollama fails in stock llama.cpp. This script
does not attempt that repair. When the probe fails, the verified file is kept
with a .incompatible suffix (so the app does not list a model it cannot load)
and the reason is written next to it. Prefer the upstream GGUF from Hugging
Face, which loads unchanged.

Requires only the Python standard library. Set HF_TOKEN for gated repositories.
"""
import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
CHUNK = 8 << 20
USER_AGENT = 'local-llm-companion-modeldownloader/2'


# --------------------------------------------------------------------------
# HTTP
# --------------------------------------------------------------------------
class _StripAuthOnHostChange(urllib.request.HTTPRedirectHandler):
    """Never forward credentials to the storage host a redirect points at."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is not None and urllib.parse.urlsplit(newurl).hostname != urllib.parse.urlsplit(req.full_url).hostname:
            new.remove_header('Authorization')
        return new


_OPENER = urllib.request.build_opener(_StripAuthOnHostChange())


def _request(url, headers=None, token=None):
    h = {'User-Agent': USER_AGENT}
    if token:
        h['Authorization'] = f'Bearer {token}'
    h.update(headers or {})
    return urllib.request.Request(url, headers=h)


def get_json(url, token=None, timeout=60):
    with _OPENER.open(_request(url, token=token), timeout=timeout) as r:
        return json.load(r)


def download(url, dest, expected_size, expected_sha256, token=None, retries=5):
    """Resumable download to dest.part; verified, then renamed to dest."""
    part = dest + '.part'
    for attempt in range(1, retries + 1):
        have = os.path.getsize(part) if os.path.exists(part) else 0
        if expected_size and have > expected_size:
            os.remove(part)
            have = 0
        if expected_size and have == expected_size:
            break
        headers = {'Range': f'bytes={have}-'} if have else {}
        try:
            with _OPENER.open(_request(url, headers, token), timeout=60) as r:
                if have and r.status != 206:
                    have = 0  # server ignored the range; start over
                mode = 'ab' if have else 'wb'
                started, done = time.time(), have
                with open(part, mode) as f:
                    while True:
                        chunk = r.read(CHUNK)
                        if not chunk:
                            break
                        f.write(chunk)
                        done += len(chunk)
                        if expected_size:
                            rate = (done - have) / max(time.time() - started, 1e-6) / 1e6
                            print(f'\r  {done / 1e9:.2f} / {expected_size / 1e9:.2f} GB '
                                  f'({done / expected_size * 100:.1f}%) {rate:.1f} MB/s', end='', flush=True)
            print()
            if not expected_size or os.path.getsize(part) == expected_size:
                break
        except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
            print(f'\n  interrupted ({e}); retry {attempt}/{retries} resumes from '
                  f'{os.path.getsize(part) if os.path.exists(part) else 0} bytes')
            time.sleep(min(2 ** attempt, 30))
    size = os.path.getsize(part)
    if expected_size and size != expected_size:
        raise SystemExit(f'Download incomplete: {size} of {expected_size} bytes. Run again to resume.')
    if expected_sha256:
        print('  verifying SHA-256 ...', end='', flush=True)
        h = hashlib.sha256()
        with open(part, 'rb') as f:
            for chunk in iter(lambda: f.read(CHUNK), b''):
                h.update(chunk)
        if h.hexdigest() != expected_sha256:
            os.remove(part)
            raise SystemExit(f'\nChecksum mismatch: got {h.hexdigest()}, the source publishes '
                             f'{expected_sha256}. The partial file was removed; run again.')
        print(' ok')
    else:
        print('  warning: the source publishes no checksum; only the size was verified')
    os.replace(part, dest)


# --------------------------------------------------------------------------
# Sources
# --------------------------------------------------------------------------
def hf_file(repo, filename, token):
    """URL, size and SHA-256 of one file in a Hugging Face repository."""
    info = get_json(f'https://huggingface.co/api/models/{repo}/tree/main?recursive=true', token)
    entry = next((f for f in info if f.get('path') == filename), None)
    if entry is None:
        ggufs = sorted(f['path'] for f in info if f.get('path', '').endswith('.gguf'))
        raise SystemExit(f'{filename} is not in {repo}. GGUF files there:\n  ' + '\n  '.join(ggufs))
    lfs = entry.get('lfs') or {}
    url = f'https://huggingface.co/{repo}/resolve/main/{urllib.parse.quote(filename)}'
    return url, lfs.get('size') or entry.get('size'), lfs.get('oid')


def ollama_model(reference):
    """URL, size and SHA-256 of an Ollama registry model layer."""
    name, _, tag = reference.partition(':')
    tag = tag or 'latest'
    path = name if '/' in name else f'library/{name}'
    manifest = get_json(f'https://registry.ollama.ai/v2/{path}/manifests/{tag}')
    layer = next((l for l in manifest.get('layers', [])
                  if l.get('mediaType') == 'application/vnd.ollama.image.model'), None)
    if layer is None:
        raise SystemExit(f'{reference} has no model layer in its manifest.')
    digest = layer['digest']
    return (f'https://registry.ollama.ai/v2/{path}/blobs/{digest}', layer.get('size'),
            digest.split(':', 1)[1] if digest.startswith('sha256:') else None)


# --------------------------------------------------------------------------
# Inspection
# --------------------------------------------------------------------------
_FIXED = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}


def inspect_gguf(path):
    """Header facts that explain load failures, read without loading weights."""
    facts = {'tower_tensors': 0, 'tensors': 0, 'chat_template': False}
    with open(path, 'rb') as f:
        if f.read(4) != b'GGUF':
            return {'error': 'not a GGUF file'}
        f.read(4)
        n_tensors, n_kv = struct.unpack('<QQ', f.read(16))
        facts['tensors'] = n_tensors

        def string():
            (n,) = struct.unpack('<Q', f.read(8))
            return f.read(n).decode('utf-8', 'replace')

        def skip(t):
            if t in _FIXED:
                f.seek(_FIXED[t], 1)
            elif t == 8:
                (n,) = struct.unpack('<Q', f.read(8))
                f.seek(n, 1)
            elif t == 9:
                et, n = struct.unpack('<IQ', f.read(12))
                if et in _FIXED:
                    f.seek(_FIXED[et] * n, 1)
                else:
                    for _ in range(n):
                        skip(et)
                return n
            else:
                raise ValueError(f'unknown GGUF type {t}')
            return None

        vocab = None
        for _ in range(n_kv):
            key = string()
            (t,) = struct.unpack('<I', f.read(4))
            if key == 'general.architecture' and t == 8:
                facts['architecture'] = string()
            elif key == 'tokenizer.chat_template':
                facts['chat_template'] = True
                skip(t)
            else:
                n = skip(t)
                if key == 'tokenizer.ggml.tokens':
                    vocab = n
        facts['vocab'] = vocab
        for _ in range(n_tensors):
            name = string()
            (n_dims,) = struct.unpack('<I', f.read(4))
            dims = struct.unpack(f'<{n_dims}Q', f.read(8 * n_dims))
            f.seek(12, 1)
            if name.startswith(('v.', 'a.', 'mm.')):
                facts['tower_tensors'] += 1
            if name == 'token_embd.weight' and n_dims >= 2:
                facts['embedding_rows'] = dims[1]
    return facts


FIT_TOOL = 'llama-fit-params.exe' if os.name == 'nt' else 'llama-fit-params'


def default_runtime_bin():
    """The runtime the app itself uses, in the app's own order: the explicit
    server override, the project's built runtime (runtime/bin next to models/),
    then a llama.cpp found on PATH. None when there is no runtime to probe with."""
    override = os.environ.get('COMPANION_LLAMA_SERVER_BIN')
    candidates = []
    if override:
        candidates.append(os.path.dirname(os.path.abspath(override)))
    candidates.append(os.path.join(os.path.dirname(HERE), 'runtime', 'bin'))
    for folder in candidates:
        if os.path.exists(os.path.join(folder, FIT_TOOL)):
            return folder
    on_path = shutil.which(FIT_TOOL)
    return os.path.dirname(on_path) if on_path else None


def probe(path, runtime_bin):
    """Ask the runtime's own loader whether it can read the file (no weights loaded)."""
    if not runtime_bin:
        return None, ('runtime probe skipped: no llama.cpp runtime found (build it with scripts/build-runtime, '
                      'set COMPANION_LLAMA_SERVER_BIN, or pass --runtime-bin)')
    tool = os.path.join(runtime_bin, FIT_TOOL)
    if not os.path.exists(tool):
        return None, f'runtime probe skipped: {tool} not found'
    p = subprocess.run([tool, '-m', path, '-lv', '4'], capture_output=True, text=True,
                       timeout=300, cwd=runtime_bin)
    if p.returncode == 0:
        return True, 'loads in the bundled llama.cpp runtime'
    errors = [line.split(' E ', 1)[-1].strip() for line in (p.stdout + p.stderr).splitlines() if ' E ' in line]
    return False, errors[0] if errors else f'the runtime refused the file (exit {p.returncode})'


def explain(facts):
    reasons = []
    if facts.get('tower_tensors'):
        reasons.append(f"{facts['tower_tensors']} vision/audio tensors are packed into the model file; "
                       'upstream llama.cpp expects them in a separate projector (mmproj) file')
    if facts.get('vocab') and facts.get('embedding_rows') and facts['vocab'] != facts['embedding_rows']:
        reasons.append(f"the tokenizer lists {facts['vocab']} tokens but the embedding has "
                       f"{facts['embedding_rows']} rows")
    if not facts.get('chat_template'):
        reasons.append('the file carries no chat template, so chat prompts would be formatted '
                       'for the wrong model')
    return reasons


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------
def default_folder(source, ident):
    if source == 'hf':
        name = ident.split('/', 1)[-1]
        for suffix in ('-GGUF', '-gguf', '_GGUF'):
            if name.endswith(suffix):
                name = name[: -len(suffix)]
        return name.lower()
    return ident.replace(':', '-').replace('/', '-').lower()


def fetch(url, size, sha, dest, token):
    if os.path.exists(dest):
        print(f'  {os.path.basename(dest)} is already present; verifying it')
        if size and os.path.getsize(dest) != size:
            raise SystemExit(f'{dest} exists with the wrong size; move it aside and run again.')
        os.replace(dest, dest + '.part')
    download(url, dest, size, sha, token)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('source', choices=['hf', 'ollama'])
    ap.add_argument('ident', help='Hugging Face repo (owner/name) or Ollama model[:tag]')
    ap.add_argument('file', nargs='?', help='GGUF file name in the Hugging Face repo')
    ap.add_argument('--mmproj', help='projector file in the same Hugging Face repo')
    ap.add_argument('--name', help='folder under models/ (default derived from the source)')
    ap.add_argument('--models-dir', default=HERE)
    ap.add_argument('--runtime-bin', default=None,
                    help='folder holding llama-fit-params (default: the runtime the app uses)')
    args = ap.parse_args()
    token = os.environ.get('HF_TOKEN')

    if args.source == 'hf':
        if not args.file:
            ap.error('hf needs the GGUF file name')
        url, size, sha = hf_file(args.ident, args.file, token)
        filename = args.file
    else:
        if args.mmproj:
            ap.error('--mmproj applies to Hugging Face repositories only')
        url, size, sha = ollama_model(args.ident)
        filename = default_folder('ollama', args.ident) + '.gguf'
        token = None

    folder = os.path.join(args.models_dir, args.name or default_folder(args.source, args.ident))
    os.makedirs(folder, exist_ok=True)
    dest = os.path.join(folder, filename)
    print(f'{args.source}: {args.ident} -> {dest}')
    print(f'  {size / 1e9:.2f} GB, sha256 {sha or "not published"}' if size else f'  sha256 {sha}')
    fetch(url, size, sha, dest, token)

    if args.mmproj:
        m_url, m_size, m_sha = hf_file(args.ident, args.mmproj, token)
        m_dest = os.path.join(folder, args.mmproj)
        print(f'projector: {args.mmproj} ({m_size / 1e9:.2f} GB)')
        fetch(m_url, m_size, m_sha, m_dest, token)

    facts = inspect_gguf(dest)
    print(f"  architecture {facts.get('architecture')}, {facts.get('tensors')} tensors, "
          f"chat template {'present' if facts.get('chat_template') else 'MISSING'}")
    ok, detail = probe(dest, args.runtime_bin or default_runtime_bin())
    reasons = explain(facts)
    if ok is False:
        blocked = dest + '.incompatible'
        os.replace(dest, blocked)
        report = os.path.join(folder, 'INCOMPATIBLE.txt')
        with open(report, 'w', encoding='utf-8') as f:
            f.write(f'{os.path.basename(dest)} was downloaded and verified, but the bundled llama.cpp '
                    f'runtime cannot load it.\n\nRuntime says: {detail}\n\n')
            if reasons:
                f.write('What differs from an upstream GGUF:\n' + ''.join(f'  - {r}\n' for r in reasons) + '\n')
            if args.source == 'ollama':
                f.write('This is Ollama\'s own packaging. Ollama repairs such files in memory when it loads '
                        'them, which is why it works there. Download the upstream GGUF from Hugging Face '
                        'instead (python modeldownloader.py hf <owner/repo> <file.gguf>).\n')
        print(f'\nNOT LOADABLE: {detail}')
        for r in reasons:
            print(f'  - {r}')
        print(f'Kept as {blocked} so the app does not list it; details in {report}')
        sys.exit(2)
    if ok is None:
        print(f'  {detail}')
    else:
        print(f'  {detail}')
    if not facts.get('chat_template'):
        print('  warning: ' + explain({'chat_template': False})[0])
    print('done')


if __name__ == '__main__':
    main()
