#!/usr/bin/env bash
# Build (first run) and start Companion on Linux and macOS: the counterpart of
# run.ps1 (Windows), with the same steps in the same order. Keep the terminal
# open and press Ctrl+C once to stop the app and the model server.

# Sourced into a shell, `set -e` and `exit` would act on that shell, so this
# check comes before them.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then
    echo "Start Companion with ./run.sh (or bash run.sh); do not source it." >&2
    return 2
fi
set -euo pipefail

case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*)
        echo 'On Windows, start Companion with .\run.ps1 in PowerShell.' >&2
        exit 2
        ;;
esac

# The project folder, also when started through a symlink (portable: macOS
# before 12.3 has no `readlink -f`).
source_path="${BASH_SOURCE[0]}"
while [ -L "$source_path" ]; do
    link_dir="$(cd "$(dirname "$source_path")" && pwd)"
    source_path="$(readlink "$source_path")"
    case "$source_path" in /*) ;; *) source_path="$link_dir/$source_path" ;; esac
done
repo_root="$(cd "$(dirname "$source_path")" && pwd)"
frontend_dir="$repo_root/frontend"
backend_dir="$repo_root/backend"

fail() {
    echo "error: $*" >&2
    exit 1
}
need() { command -v "$1" >/dev/null 2>&1 || fail "$1 was not found on PATH. $2 See Prerequisites in README.md."; }

need npm 'Install Node.js 24 or newer (it includes npm).'
need cargo 'Install Rust (rustup) and open a new terminal.'

# The llama.cpp runtime is part of the project: built once from the pinned
# commit (runtime/llama.cpp.lock.json), then reused. An explicit override
# (COMPANION_LLAMA_SERVER_BIN) skips the build.
runtime_server="$repo_root/runtime/bin/llama-server"
if [ -z "${COMPANION_LLAMA_SERVER_BIN:-}" ] && [ ! -f "$runtime_server" ]; then
    echo 'The llama.cpp runtime is not built yet; building it now (one time).'
    # Through bash, so a checkout without the executable bit still builds.
    bash "$repo_root/scripts/build-runtime.sh" || fail 'The runtime build failed. See the output above.'
    [ -f "$runtime_server" ] || fail 'The runtime build did not produce llama-server. See the output above.'
fi

(
    cd "$frontend_dir"
    if [ ! -d node_modules ]; then
        npm ci || fail 'Frontend dependency installation failed.'
    fi
    npm run build || fail 'Frontend build failed. The backend was not started with stale UI files.'
)

# The backend serves the compiled UI and API from one process/port. The
# address applies to this script's process only, as run.ps1 restores it.
cd "$backend_dir"
export COMPANION_ADDR='127.0.0.1:5173'
cargo run --bin companion-backend || fail 'Companion stopped with an error. See the diagnostic above.'
