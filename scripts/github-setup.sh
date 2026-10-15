#!/bin/bash
# The settings of the catalog's repository on GitHub that its files can't hold. Idempotent: it
# changes only what differs.
#
#   scripts/github-setup.sh                         # a dry run: what would change
#   scripts/github-setup.sh --apply                 # change it
#   scripts/github-setup.sh --apply --deploy-key    # and make the index's deploy key (or a new one)
#
# - Private vulnerability reporting on.
# - The index's deploy key (--deploy-key): an ed25519 key made in a temporary folder, added to the
#   repository as a deploy key with write access, its private half saved as the Actions secret
#   INDEX_DEPLOY_KEY and deleted here; an older key of the same title is removed after it.
#   The Catalog workflow pushes index.json to main with it.
# - The ruleset of .github/rulesets/main.json: main takes changes only through pull requests, and
#   deploy keys bypass it. It is set only when the index can still get through: the workflow on main
#   pushes with INDEX_DEPLOY_KEY, the secret is there, and so is a deploy key with write access —
#   otherwise the next publish would fail at Index.
#
# Needs gh, logged in as an admin of the repository (the repo scope), jq and ssh-keygen.
set -euo pipefail

REPO=${FLUX_PLUGINS_GITHUB_REPO:-egor4geev/flux-plugins}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SECRET=INDEX_DEPLOY_KEY
KEY_TITLE="Catalog index (CI)"
WORKFLOW=.github/workflows/release.yml

APPLY=false
DEPLOY_KEY=false
for arg in "$@"; do
    case "$arg" in
        --apply) APPLY=true ;;
        --deploy-key) DEPLOY_KEY=true ;;
        *)
            echo "usage: $0 [--apply] [--deploy-key]" >&2
            exit 2
            ;;
    esac
done

for tool in gh jq ssh-keygen; do
    command -v "$tool" >/dev/null || {
        echo "$tool is needed" >&2
        exit 1
    }
done
if [ "$(gh api "repos/$REPO" --jq .permissions.admin)" != true ]; then
    echo "gh's account isn't an admin of $REPO (gh auth status)" >&2
    exit 1
fi

if $APPLY; then
    echo "$REPO: applying"
else
    echo "$REPO: a dry run (--apply changes it)"
fi
changes=0

# Prints what changes; with --apply, runs the command after it.
change() {
    local what=$1
    shift
    changes=$((changes + 1))
    echo "  → $what"
    if $APPLY; then
        "$@" >/dev/null
    fi
}

# --- Private vulnerability reporting ---
echo "Private vulnerability reporting"
if [ "$(gh api "repos/$REPO/private-vulnerability-reporting" --jq .enabled)" = true ]; then
    echo "  on"
else
    echo "  off → on"
    change "turn it on" gh api -X PUT "repos/$REPO/private-vulnerability-reporting"
fi

# --- The index's deploy key ---
echo "Deploy key \"$KEY_TITLE\" and the secret $SECRET"
keys=$(gh api "repos/$REPO/keys?per_page=100" --paginate --jq '.[] | {id, title, read_only}' | jq -s .)
old_keys=$(jq -r --arg title "$KEY_TITLE" '.[] | select(.title == $title) | .id' <<<"$keys")
has_secret() { gh api "repos/$REPO/actions/secrets/$SECRET" --silent 2>/dev/null; }
if [ -n "$old_keys" ] && has_secret; then
    echo "  both there (--deploy-key makes a new pair)"
else
    echo "  missing: run with --deploy-key"
fi
# The private key lives only in this folder, removed however the script ends.
KEY_DIR=""
trap '[ -z "$KEY_DIR" ] || rm -rf "$KEY_DIR"' EXIT
make_key() {
    KEY_DIR=$(mktemp -d)
    ssh-keygen -q -t ed25519 -N "" -C "$KEY_TITLE" -f "$KEY_DIR/key"
    local id
    id=$(gh api -X POST "repos/$REPO/keys" -f title="$KEY_TITLE" -f key="$(cat "$KEY_DIR/key.pub")" \
        -F read_only=false --jq .id)
    gh secret set "$SECRET" --repo "$REPO" <"$KEY_DIR/key"
    rm -rf "$KEY_DIR"
    KEY_DIR=""
    for old in $old_keys; do
        [ "$old" = "$id" ] || gh api -X DELETE "repos/$REPO/keys/$old"
    done
}
if $DEPLOY_KEY; then
    change "make a key pair: the public half as a deploy key with write access, the private one as $SECRET${old_keys:+, then remove the old key}" make_key
fi

# --- The ruleset ---
# A ruleset as GitHub returns it, cut down to what the file says (the rules of the file's types with
# the parameters the file names, the conditions, the bypass list, the enforcement), keys sorted.
current_ruleset() { # $1: the ruleset's JSON from GitHub, $2: the file's JSON
    jq -S -n --argjson current "$1" --argjson wanted "$2" '{
        target: $current.target,
        enforcement: $current.enforcement,
        conditions: $current.conditions,
        bypass_actors: [$current.bypass_actors[]? | {actor_type, bypass_mode}
            + (if .actor_type == "DeployKey" then {} else {actor_id} end)] | sort_by(.actor_type),
        rules: [($current.rules // [])[] as $rule
            | ($wanted.rules | map(select(.type == $rule.type)) | first) as $want
            | {type: $rule.type}
            + (if $want.parameters then {parameters: ($rule.parameters // {}
                | with_entries(select(.key as $key | $want.parameters | has($key))))} else {} end)]
            | sort_by(.type)
    }'
}
wanted_ruleset() { # $1: the file's JSON
    jq -S '{target, enforcement, conditions,
        bypass_actors: [.bypass_actors[]? | {actor_type, bypass_mode}
            + (if .actor_type == "DeployKey" then {} else {actor_id} end)] | sort_by(.actor_type),
        rules: [.rules[] | {type} + (if .parameters then {parameters} else {} end)] | sort_by(.type)}' \
        <<<"$1"
}

file=$ROOT/.github/rulesets/main.json
ruleset=$(jq . "$file")
name=$(jq -r .name <<<"$ruleset")
echo "Ruleset \"$name\" (${file#"$ROOT"/})"
# The index gets through only with the key: on main's workflow, as the secret, as a deploy key.
blocked=""
if ! gh api "repos/$REPO/contents/$WORKFLOW?ref=main" --jq .content 2>/dev/null |
    base64 --decode 2>/dev/null | grep -q "secrets\.$SECRET"; then
    blocked="$WORKFLOW on main doesn't push with $SECRET yet (merge the pull request that brings it)"
elif ! $DEPLOY_KEY && ! has_secret; then
    blocked="there is no secret $SECRET (--deploy-key makes it)"
elif ! $DEPLOY_KEY && [ -z "$old_keys" ]; then
    blocked="there is no deploy key \"$KEY_TITLE\" (--deploy-key makes it)"
elif ! $DEPLOY_KEY &&
    [ "$(jq -r --arg title "$KEY_TITLE" 'map(select(.title == $title)) | all(.read_only)' <<<"$keys")" = true ]; then
    blocked="the deploy key \"$KEY_TITLE\" is read-only (--deploy-key makes a new one)"
fi
id=$(gh api "repos/$REPO/rulesets?per_page=100" --paginate --jq '.[] | {id, name}' |
    jq -s -r --arg name "$name" 'map(select(.name == $name)) | first | .id // empty')
if [ -n "$blocked" ]; then
    echo "  not now: $blocked — the ruleset would stop the index's push to main."
elif [ -z "$id" ]; then
    change "create it" gh api -X POST "repos/$REPO/rulesets" --input - <<<"$ruleset"
else
    now=$(current_ruleset "$(gh api "repos/$REPO/rulesets/$id")" "$ruleset")
    want=$(wanted_ruleset "$ruleset")
    if [ "$now" = "$want" ]; then
        echo "  as it should be"
    else
        diff <(echo "$now") <(echo "$want") | sed 's/^/  /' || true
        change "update it (ruleset $id)" gh api -X PUT "repos/$REPO/rulesets/$id" --input - <<<"$ruleset"
    fi
fi

if [ "$changes" -eq 0 ]; then
    echo "Nothing to change."
elif ! $APPLY; then
    echo "$changes change(s): run with --apply."
fi
