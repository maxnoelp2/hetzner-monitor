#!/usr/bin/env bash
# M2 smoke test: watch alerts of a REAL Hetzner Cloud project while you power a server off and on.
#
# Use a dedicated TEST project/server — never power off production servers for this.
#
# Usage (secrets only via environment, never as arguments):
#   read -rs HM_API_KEY && export HM_API_KEY          # hetzner-monitor key (write scope)
#   read -rs HCLOUD_TOKEN && export HCLOUD_TOKEN      # READ-only token of the test project
#   scripts/smoke-m2.sh                               # connects the project, then watches alerts
#   HM_PROJECT_ID=<uuid> scripts/smoke-m2.sh          # watch an already connected project instead
#
# Then, in the Hetzner Console: power the test server off → within ~3 min `firing`;
# power it on → within ~3 min `resolved`. Stop with Ctrl+C. The project stays connected
# (delete it with `DELETE /api/v1/projects/{id}`). Python 3.9 compatible; nothing secret is printed.
set -euo pipefail

: "${HM_API_KEY:?export HM_API_KEY (hetzner-monitor API key with write scope)}"
HM_URL="${HM_URL:-http://127.0.0.1:8080}"
INTERVAL="${SMOKE_INTERVAL:-15}"

auth_header() { printf 'Authorization: Bearer %s' "$HM_API_KEY"; }
api() { # method path [curl args...] — secrets go via header file / stdin, not argv
  local method="$1" path="$2"; shift 2
  curl -sS -X "$method" -H @<(auth_header) -H 'Content-Type: application/json' "$@" "$HM_URL/api/v1$path"
}

preflight="$(api GET "/alerts?per_page=1" -o /dev/null -w '%{http_code}')"
if [[ "$preflight" != "200" ]]; then
  echo "✗ GET /alerts returned HTTP $preflight — is the running stack M2 and HM_API_KEY valid?"
  echo "  rebuild: docker compose -f deploy/docker-compose.yml up --build -d"
  exit 1
fi

project_id="${HM_PROJECT_ID:-}"
if [[ -z "$project_id" ]]; then
  : "${HCLOUD_TOKEN:?export HCLOUD_TOKEN (read-only token of the TEST project) or set HM_PROJECT_ID}"
  export SMOKE_PROJECT="${SMOKE_PROJECT:-smoke-m2-$(date +%Y%m%d-%H%M%S)}"
  echo "→ connecting project '$SMOKE_PROJECT' at $HM_URL"
  response="$(python3 -I -c 'import json,os; print(json.dumps({"name": os.environ["SMOKE_PROJECT"], "token": os.environ["HCLOUD_TOKEN"]}))' \
    | api POST /projects --data-binary @- -w '\n%{http_code}')"
  status="${response##*$'\n'}"; body="${response%$'\n'*}"
  if [[ "$status" != "201" ]]; then
    echo "✗ connect failed: HTTP $status"; echo "$body"; exit 1
  fi
  project_id="$(python3 -I -c 'import json,sys; print(json.loads(sys.argv[1])["project"]["id"])' "$body")"
fi
echo "✓ project $project_id"

echo "→ servers:"
api GET "/projects/$project_id/resources?kind=server" | python3 -I -c '
import json, sys
for r in json.load(sys.stdin)["resources"]:
    if not r["deleted_at"]:
        print("    {:>10}  {}".format(r["hetzner_id"], r["name"]))
'

read -r -p "Are these servers in a dedicated TEST project that you may power off? [y/N] " answer
if [[ "$answer" != "y" && "$answer" != "Y" ]]; then
  if [[ -z "${HM_PROJECT_ID:-}" ]]; then
    api DELETE "/projects/$project_id" -o /dev/null && echo "→ project disconnected again"
  fi
  echo "✗ aborted — connect a test project (its own READ token) instead"; exit 1
fi

echo "→ watching alerts every ${INTERVAL}s (Ctrl+C to stop). Power the test server off now."
last=""
while true; do
  response="$(api GET "/alerts?project_id=$project_id&per_page=100" -w '\n%{http_code}')"
  status="${response##*$'\n'}"; body="${response%$'\n'*}"
  if [[ "$status" != "200" ]]; then
    echo "[$(date +%H:%M:%S)] ✗ GET /alerts: HTTP $status"; sleep "$INTERVAL"; continue
  fi
  snapshot="$(printf '%s' "$body" | python3 -I -c '
import json, sys
body = json.load(sys.stdin)
lines = []
for a in sorted(body.get("alerts", []), key=lambda a: (a["check_type"], a["message"])):
    if a["state"] == "resolved" and a["check_type"] == "resource_lifecycle":
        continue
    lines.append("    {:<9} {:<8} {:<18} {}".format(a["state"], a["severity"], a["check_type"], a["message"]))
print("\n".join(lines) if lines else "    (no alerts)")
')"
  if [[ "$snapshot" != "$last" ]]; then
    echo "[$(date +%H:%M:%S)] alerts changed:"
    echo "$snapshot"
    last="$snapshot"
  fi
  sleep "$INTERVAL"
done
