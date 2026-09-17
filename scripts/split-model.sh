#!/usr/bin/env bash
# Split a single-file GGUF into parts small enough for Git LFS, write them into
# models/<folder>/, and check that the runtime still loads the result.
#
# GitHub rejects any file over 100 MB on a normal push, and Git LFS stores at
# most 2 GB per object on Free and Pro (4 GB Team, 5 GB Enterprise Cloud). A
# quantised model is larger than both, so a model committed to this repository
# is split first: llama.cpp reads a split GGUF natively as long as every part
# stays in one folder, which is how models/gemma-4-e4b-it-qat/ was made (night
# decisions 51 and 55).
#
# The split uses llama.cpp's own llama-gguf-split, taken from the runtime this
# project builds (runtime/bin), so the parts match the loader that reads them.
# Part sizes are measured afterwards instead of assumed. gguf-split counts only
# tensor bytes against --split-max-size, while the first part also carries the
# whole metadata block on top of them, so that part can land over the limit
# while gguf-split still reports it as within budget: a large tokenizer is
# enough to do it. Left unchecked, that surfaces only when the push is refused.
#
# Usage:
#   scripts/split-model.sh model.gguf
#   scripts/split-model.sh model.gguf --name gemma-4-e2b-it
#   scripts/split-model.sh model.gguf --max-size 1500M --track
#   scripts/split-model.sh model.gguf --dry-run
#
# Options:
#   --name NAME        folder under models/ (default: the file name, lowercased,
#                      without .gguf). The folders in this repository drop the
#                      quantisation suffix, so pass --name to match them.
#   --out DIR          write the parts here instead of models/<name>
#   --max-size N(M|G)  size per part, in the decimal units gguf-split itself
#                      reads (M = 1000*1000, G = 1000*1000*1000). Default 1500M.
#   --lfs-limit N(M|G) largest part Git LFS will take (default 2G: GitHub Free
#                      and Pro. Team is 4G, Enterprise Cloud 5G.)
#   --track            git lfs track the parts and add the models/ exception to
#                      .gitignore, so the parts can actually be committed
#   --no-probe         skip asking the runtime to load the parts
#   --split-bin PATH   the llama-gguf-split to use
#   --runtime-bin DIR  folder holding the runtime's tools, searched first for
#                      both llama-gguf-split and llama-fit-params
#   --force            replace the .gguf files in an existing output folder
#   --dry-run          print the plan, split nothing
set -euo pipefail

name=""
out_dir=""
max_size="1500M"
lfs_limit="2G"
track=0
probe=1
split_bin=""
runtime_bin=""
force=0
dry_run=0
model=""

while [ $# -gt 0 ]; do
    case "$1" in
        --name) name="$2"; shift 2 ;;
        --out) out_dir="$2"; shift 2 ;;
        --max-size) max_size="$2"; shift 2 ;;
        --lfs-limit) lfs_limit="$2"; shift 2 ;;
        --track) track=1; shift ;;
        --no-probe) probe=0; shift ;;
        --split-bin) split_bin="$2"; shift 2 ;;
        --runtime-bin) runtime_bin="$2"; shift 2 ;;
        --force) force=1; shift ;;
        --dry-run) dry_run=1; shift ;;
        -h|--help) sed -n '2,42p' "$0"; exit 0 ;;
        -*) echo "Unknown option: $1" >&2; exit 2 ;;
        *)
            [ -z "$model" ] || { echo "Split one model at a time; got '$model' and '$1'." >&2; exit 2; }
            model="$1"; shift
            ;;
    esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

step() { printf '==> %s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
fail() { echo "error: $*" >&2; exit 1; }

# stat's size flag differs between GNU (Linux) and BSD (macOS).
file_size() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; }

# Both sides of the limit check use gguf-split's own decimal units, so the
# number passed as --max-size means the same thing as the number compared
# against --lfs-limit.
to_bytes() {
    local value="$1" number unit
    number="${value%[MmGg]}"
    unit="$(printf '%s' "${value#"$number"}" | tr '[:lower:]' '[:upper:]')"
    case "$number" in ''|*[!0-9]*) fail "not a size: '$value' (use for example 1500M or 2G)" ;; esac
    case "$unit" in
        M) echo $((number * 1000 * 1000)) ;;
        G) echo $((number * 1000 * 1000 * 1000)) ;;
        *) fail "not a size: '$value' (use for example 1500M or 2G)" ;;
    esac
}

# Git LFS limits are quoted in decimal GB; file managers and the decision log
# show MiB. Printing both keeps either comparison honest.
human() { awk -v b="$1" 'BEGIN { printf "%8.0f MiB (%5.2f GB)", b / 1048576, b / 1000000000 }'; }

# ---------------------------------------------------------------------------
# The model to split
# ---------------------------------------------------------------------------
[ -n "$model" ] || fail "no model given. Usage: scripts/split-model.sh <model.gguf> [options]"
[ -f "$model" ] || fail "no such file: $model"
model="$(cd "$(dirname "$model")" && pwd)/$(basename "$model")"

base="$(basename "$model")"
case "$base" in
    *-[0-9][0-9][0-9][0-9][0-9]-of-[0-9][0-9][0-9][0-9][0-9].gguf)
        fail "$base is already one part of a split model; pass the single-file GGUF instead" ;;
esac
[ "$(head -c 4 "$model")" = "GGUF" ] || fail "$base does not start with the GGUF magic, so it is not a GGUF file"

prefix_name="${base%.gguf}"
[ "$prefix_name" != "$base" ] || fail "$base does not end in .gguf"
[ -n "$name" ] || name="$(printf '%s' "$prefix_name" | tr '[:upper:]' '[:lower:]')"
[ -n "$out_dir" ] || out_dir="$repo_root/models/$name"

# ---------------------------------------------------------------------------
# Tools, found in the order the app itself uses (models/modeldownloader.py
# resolves the probe tool the same way): an explicit override, the project's
# built runtime, then a llama.cpp on PATH.
# ---------------------------------------------------------------------------
split_tool="llama-gguf-split"
fit_tool="llama-fit-params"
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) split_tool="$split_tool.exe"; fit_tool="$fit_tool.exe" ;; esac

find_tool() {
    local tool="$1" candidate
    if [ -n "${runtime_bin:-}" ] && [ -x "$runtime_bin/$tool" ]; then
        echo "$runtime_bin/$tool"; return 0
    fi
    if [ -n "${COMPANION_LLAMA_SERVER_BIN:-}" ]; then
        candidate="$(dirname "$COMPANION_LLAMA_SERVER_BIN")/$tool"
        if [ -x "$candidate" ]; then echo "$candidate"; return 0; fi
    fi
    if [ -x "$repo_root/runtime/bin/$tool" ]; then
        echo "$repo_root/runtime/bin/$tool"; return 0
    fi
    command -v "$tool" 2>/dev/null || return 1
}

if [ -n "$split_bin" ]; then
    [ -x "$split_bin" ] || fail "--split-bin $split_bin is not executable"
else
    split_bin="$(find_tool "$split_tool" || true)"
    [ -n "$split_bin" ] || fail "$split_tool was not found. Build the runtime with scripts/build-runtime.sh, or pass --split-bin."
fi

limit_bytes="$(to_bytes "$lfs_limit")"
max_bytes="$(to_bytes "$max_size")"
[ "$max_bytes" -le "$limit_bytes" ] || fail "--max-size $max_size is above --lfs-limit $lfs_limit, so every part would be rejected"

source_size="$(file_size "$model")"

step "Splitting $base"
echo "    size:      $(human "$source_size")"
echo "    into:      $out_dir"
echo "    parts of:  up to $max_size, limit $lfs_limit per part"
echo "    tool:      $split_bin"

if [ $dry_run -eq 1 ]; then
    step "Dry run: nothing was written"
    echo "    $split_bin --split --split-max-size $max_size $model $out_dir/$prefix_name"
    exit 0
fi

# ---------------------------------------------------------------------------
# Split
# ---------------------------------------------------------------------------
if [ -d "$out_dir" ]; then
    # An array, not a string: a project folder can sit under a path with spaces.
    existing=()
    for old in "$out_dir"/*.gguf; do
        if [ -f "$old" ]; then existing+=("$old"); fi
    done
    if [ ${#existing[@]} -gt 0 ]; then
        [ $force -eq 1 ] || fail "$out_dir already holds .gguf files; move them aside or pass --force"
        step "Replacing the .gguf files already in $out_dir"
        for old in "${existing[@]}"; do echo "    removing $(basename "$old")"; rm -f "$old"; done
    fi
fi
mkdir -p "$out_dir"

"$split_bin" --split --split-max-size "$max_size" "$model" "$out_dir/$prefix_name"

parts=()
for part in "$out_dir/$prefix_name"-[0-9]*-of-[0-9]*.gguf; do
    if [ -f "$part" ]; then parts+=("$part"); fi
done
[ ${#parts[@]} -gt 0 ] || fail "the split produced no parts in $out_dir"

# ---------------------------------------------------------------------------
# Check the parts before anything is committed
# ---------------------------------------------------------------------------
step "Parts written"
total=0
oversized=0
for part in "${parts[@]}"; do
    size="$(file_size "$part")"
    total=$((total + size))
    if [ "$size" -gt "$limit_bytes" ]; then
        oversized=$((oversized + 1))
        printf '    %-54s %s  <-- over the %s limit\n' "$(basename "$part")" "$(human "$size")" "$lfs_limit"
    else
        printf '    %-54s %s\n' "$(basename "$part")" "$(human "$size")"
    fi
done
printf '    %-54s %s\n' "${#parts[@]} parts, total" "$(human "$total")"

# Every part repeats the metadata, so the parts together are never smaller than
# the file they came from. Less than that means a part failed to write.
[ "$total" -ge "$source_size" ] || fail "the parts total $(human "$total") but the source is $(human "$source_size"); the split is incomplete"
[ "$oversized" -eq 0 ] || fail "$oversized part(s) exceed $lfs_limit. Split again with a smaller --max-size. The first part carries the model's metadata as well as its tensors, so it runs larger than the rest."

# ---------------------------------------------------------------------------
# Ask the runtime to load them
# ---------------------------------------------------------------------------
if [ $probe -eq 1 ]; then
    fit_bin="$(find_tool "$fit_tool" || true)"
    if [ -z "$fit_bin" ]; then
        echo "warning: $fit_tool was not found, so the parts were not load-tested." >&2
        echo "         Build the runtime with scripts/build-runtime.sh, or pass --runtime-bin/--no-probe." >&2
    else
        step "Loading the parts with $(basename "$fit_bin")"
        probe_log="$(mktemp)"
        # Run from the tool's own folder: the runtime binaries look for their
        # libraries next to themselves.
        if ( cd "$(dirname "$fit_bin")" && "$fit_bin" -m "${parts[0]}" -lv 4 ) >"$probe_log" 2>&1; then
            echo "    the runtime reads $(basename "${parts[0]}") and the parts beside it"
            rm -f "$probe_log"
        else
            echo "--- runtime output ---" >&2
            grep ' E ' "$probe_log" >&2 || tail -n 20 "$probe_log" >&2
            rm -f "$probe_log"
            fail "the runtime could not load the split model. The parts were kept in $out_dir so you can inspect them."
        fi
    fi
fi

# ---------------------------------------------------------------------------
# Make the parts committable
# ---------------------------------------------------------------------------
if [ $track -eq 1 ]; then
    have git || fail "git is required for --track"
    git -C "$repo_root" lfs version >/dev/null 2>&1 || fail "git-lfs is not installed (see https://git-lfs.com); the parts are too large for a normal push"
    case "$out_dir" in
        "$repo_root"/*) rel="${out_dir#"$repo_root"/}" ;;
        *) fail "--track only works for a folder inside the repository, not $out_dir" ;;
    esac

    step "Tracking $rel/*.gguf with Git LFS"
    git -C "$repo_root" lfs install --local >/dev/null
    git -C "$repo_root" lfs track "$rel/*.gguf" >/dev/null

    # .gitignore ignores /models/* and *.gguf, so the folder needs an exception
    # or `git add` silently does nothing.
    ignore="$repo_root/.gitignore"
    if ! grep -qxF "!/$rel/" "$ignore" 2>/dev/null; then
        step "Adding the .gitignore exception for $rel/"
        printf '!/%s/\n!/%s/*.gguf\n' "$rel" "$rel" >> "$ignore"
    fi

    for part in "${parts[@]}"; do
        if git -C "$repo_root" check-ignore -q "$part"; then
            fail "git still ignores $(basename "$part"); check the rules in .gitignore"
        fi
    done
    echo "    git add $rel .gitattributes .gitignore && git commit"
    echo "    every clone then pays $(human "$total") of Git LFS bandwidth"
else
    step "Next"
    echo "    the parts are not committable yet: .gitignore ignores /models/* and *.gguf,"
    echo "    and a part this size needs Git LFS. Re-run with --track to set both up."
fi
