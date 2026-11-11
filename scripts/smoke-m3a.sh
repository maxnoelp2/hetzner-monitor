#!/usr/bin/env bash
# M3a smoke test, READ-ONLY: proves the metrics tier against real Hetzner data without touching
# or loading any server. A temporary custom `cpu` rule with a very low threshold (default
# 1 % for 120 s) is created for one server; it should fire from the real series within a few
# minutes. The rule (and a project this script connected) is removed again on exit.
#
# Usage (secrets only via environment, never as arguments):
#   read -rs HM_API_KEY && export HM_API_KEY          # hetzner-monitor key (write scope)
#   read -rs HCLOUD_TOKEN && export HCLOUD_TOKEN      # READ-only Hetzner token
#   scripts/smoke-m3a.sh
#   HM_PROJECT_ID=<uuid> scripts/smoke-m3a.sh         # use an already connected project
# Optional: SMOKE_THRESHOLD (default 1), SMOKE_FOR_S (default 120), SMOKE_TIMEOUT_S (default 600).
# Python 3.9 compatible; nothing secret is printed.
set -euo pipefail

: "${HM_API_KEY:?export HM_API_KEY (hetzner-monitor API key with write scope)}"
HM_URL="${HM_URL:-http://127.0.0.1:8080}"
THRESHOLD="${SMOKE_THRESHOLD:-1}"
FOR_S="${SMOKE_FOR_S:-120}"
TIMEOUT_S="${SMOKE_TIMEOUT_S:-600}"

auth_header() { printf 'Authorization: Bearer %s' "$HM_API_KEY"; }
api() { # method path [curl args...] — secrets go via header file / stdin, not argv
  local method="$1" path="$2"; shift 2
  curl -sS -X "$method" -H @<(auth_header) -H 'Content-Type: application/json' "$@" "$HM_URL/api/v1$path"
}
json_field() { python3 -I -c 'import json,sys; v=json.loads(sys.argv[1]); [v := v[k] for k in sys.argv[2:]]; print(v)' "$@"; }

preflight="$(api GET "/rules?per_page=1" -o /dev/null -w '%{http_code}')"
if [[ "$preflight" != "200" ]]; then
  echo "✗ GET /rules returned HTTP $preflight — is the stack running M3a and HM_API_KEY valid?"
  echo "  rebuild: docker compose -f deploy/docker-compose.yml up --build -d"
  exit 1
fi

project_id="${HM_PROJECT_ID:-}"
connected_here=""
rule_id=""
cleanup() {
  if [[ -n "$rule_id" ]]; then
    api DELETE "/rules/$rule_id" -o /dev/null && echo "→ temporary rule deleted"
  fi
  if [[ -n "$connected_here" ]]; then
    api DELETE "/projects/$project_id" -o /dev/null && echo "→ project disconnected again"
  fi
}
trap cleanup EXIT

if [[ -z "$project_id" ]]; then
  : "${HCLOUD_TOKEN:?export HCLOUD_TOKEN (read-only token) or set HM_PROJECT_ID}"
  export SMOKE_PROJECT="${SMOKE_PROJECT:-smoke-m3a-$(date +%Y%m%d-%H%M%S)}"
  echo "→ connecting project '$SMOKE_PROJECT' (read-only) at $HM_URL"
  response="$(python3 -I -c 'import json,os; print(json.dumps({"name": os.environ["SMOKE_PROJECT"], "token": os.environ["HCLOUD_TOKEN"]}))' \
    | api POST /projects --data-binary @- -w '\n%{http_code}')"
  status="${response##*$'\n'}"; body="${response%$'\n'*}"
  if [[ "$status" != "201" ]]; then
    echo "✗ connect failed: HTTP $status"; echo "$body"; exit 1
  fi
  project_id="$(json_field "$body" project id)"
  connected_here=1
fi
echo "✓ project $project_id"

echo "→ servers (nothing is changed on them — this test only reads metrics):"
servers="$(api GET "/projects/$project_id/resources?kind=server")"
python3 -I -c '
import json, sys
active = [r for r in json.loads(sys.argv[1])["resources"] if not r["deleted_at"]]
for i, r in enumerate(active, 1):
    print("    [{}] {:>10}  {}".format(i, r["hetzner_id"], r["name"]))
' "$servers"
read -r -p "Pick a server number for the temporary rule [1]: " pick
pick="${pick:-1}"
resource_id="$(python3 -I -c '
import json, sys
active = [r for r in json.loads(sys.argv[1])["resources"] if not r["deleted_at"]]
print(active[int(sys.argv[2]) - 1]["id"])
' "$servers" "$pick")"

rule_body="$(python3 -I -c '
import json, sys
print(json.dumps({"project_id": sys.argv[1], "resource_id": sys.argv[2], "name": "smoke-m3a (temporary)",
                  "check_type": "cpu", "severity": "info",
                  "params": {"threshold_pct": float(sys.argv[3]), "for_s": int(sys.argv[4])}}))
' "$project_id" "$resource_id" "$THRESHOLD" "$FOR_S")"
response="$(printf '%s' "$rule_body" | api POST /rules --data-binary @- -w '\n%{http_code}')"
status="${response##*$'\n'}"; body="${response%$'\n'*}"
if [[ "$status" != "201" ]]; then
  echo "✗ creating the temporary rule failed: HTTP $status"; echo "$body"; exit 1
fi
rule_id="$(json_field "$body" rule id)"
echo "✓ temporary rule: cpu > ${THRESHOLD} % for ${FOR_S} s (info), id $rule_id"
echo "→ waiting for the metrics tier (up to ${TIMEOUT_S}s; compare with the CPU graph in the Hetzner Console)"

deadline=$(( $(date +%s) + TIMEOUT_S ))
last=""
while (( $(date +%s) < deadline )); do
  alerts="$(api GET "/alerts?project_id=$project_id&per_page=100")"
  line="$(python3 -I -c '
import json, sys
rule = sys.argv[2]
for a in json.loads(sys.argv[1]).get("alerts", []):
    if a["rule_id"] == rule:
        print("{} | value {} | {}".format(a["state"], a["value"], a["message"]))
        break
' "$alerts" "$rule_id")"
  project="$(api GET "/projects/$project_id")"
  sched="$(python3 -I -c '
import json, sys
p = json.loads(sys.argv[1])["project"]
print("metrics every {}s, last {}, rate limit {}/{}".format(p["metrics_interval_s"], p["last_metrics_at"], p["rl_remaining"], p["rl_limit"]))
' "$project")"
  current="${line:-no alert yet} — $sched"
  if [[ "$current" != "$last" ]]; then
    echo "[$(date +%H:%M:%S)] $current"
    last="$current"
  fi
  if [[ "$line" == firing* ]]; then
    echo "✓ smoke test passed — the rule fired from real Hetzner metrics"
    exit 0
  fi
  sleep 15
done
echo "✗ no firing alert within ${TIMEOUT_S}s (an idle server below ${THRESHOLD} % would explain it; retry with SMOKE_THRESHOLD=0.1)"
exit 1
