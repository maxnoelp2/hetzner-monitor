#!/usr/bin/env bash
# M1 smoke test against a running instance and a REAL Hetzner Cloud project.
#
# Usage (secrets only via environment, never as arguments):
#   read -rs HCLOUD_TOKEN && export HCLOUD_TOKEN      # paste a READ-only Hetzner token, input hidden
#   read -rs HM_API_KEY   && export HM_API_KEY        # admin/write key from `hetzner-monitor bootstrap`
#   scripts/smoke-m1.sh [--keep]
#
# Optional: HM_URL (default http://127.0.0.1:8080), SMOKE_PROJECT (default smoke-<timestamp>).
# Without --keep the project is deleted again at the end. Nothing secret is printed.
set -euo pipefail

: "${HCLOUD_TOKEN:?export HCLOUD_TOKEN (read-only Hetzner Cloud token)}"
: "${HM_API_KEY:?export HM_API_KEY (hetzner-monitor API key with write scope)}"
HM_URL="${HM_URL:-http://127.0.0.1:8080}"
export SMOKE_PROJECT="${SMOKE_PROJECT:-smoke-$(date +%Y%m%d-%H%M%S)}"
KEEP="${1:-}"

auth_header() { printf 'Authorization: Bearer %s' "$HM_API_KEY"; }
api() { # method path [curl args...] — secrets go via header file / stdin, not argv
  local method="$1" path="$2"; shift 2
  curl -sS -X "$method" -H @<(auth_header) -H 'Content-Type: application/json' "$@" "$HM_URL/api/v1$path"
}

echo "→ connecting project '$SMOKE_PROJECT' at $HM_URL"
response="$(python3 -I -c 'import json,os; print(json.dumps({"name": os.environ["SMOKE_PROJECT"], "token": os.environ["HCLOUD_TOKEN"]}))' \
  | api POST /projects --data-binary @- -w '\n%{http_code}')"
status="${response##*$'\n'}"; body="${response%$'\n'*}"
if [[ "$status" != "201" ]]; then
  echo "✗ connect failed: HTTP $status"; echo "$body"; exit 1
fi

python3 -I -c '
import json, sys
body = json.loads(sys.argv[1]); p = body["project"]
print("✓ connected: id={id} status={status} token_hint=…{token_hint}".format(**p))
print("  rate limit: {rl_remaining}/{rl_limit} remaining".format(**p))
print("  inventory (seen / new / deleted):")
for kind, c in (body.get("sync") or {}).items():
    print("    {:<14} {:>4} / {:>4} / {:>4}".format(kind, c["seen"], c["new"], c["deleted"]))
' "$body"
project_id="$(python3 -I -c 'import json,sys; print(json.loads(sys.argv[1])["project"]["id"])' "$body")"

echo "→ resources:"
api GET "/projects/$project_id/resources" | python3 -I -c '
import json, sys
for r in json.load(sys.stdin)["resources"]:
    state = "deleted" if r["deleted_at"] else "active"
    print("    {:<14} {:>10}  {}  ({})".format(r["kind"], r["hetzner_id"], r["name"], state))
'

echo "→ token leak check"
if api GET "/projects/$project_id" | grep -qF "$HCLOUD_TOKEN"; then echo "✗ token found in API response"; exit 1; fi
if command -v docker >/dev/null && docker compose -f deploy/docker-compose.yml ps -q app >/dev/null 2>&1; then
  if docker compose -f deploy/docker-compose.yml logs app 2>&1 | grep -qF "$HCLOUD_TOKEN"; then
    echo "✗ token found in app logs"; exit 1
  fi
  echo "✓ token not in API response or app logs"
else
  echo "✓ token not in API response (app logs not checked: compose stack not found)"
fi

if [[ "$KEEP" != "--keep" ]]; then
  api DELETE "/projects/$project_id" -o /dev/null -w '' && echo "→ project deleted again (use --keep to keep it)"
fi
echo "✓ smoke test passed — compare the inventory above with the Hetzner Console"
