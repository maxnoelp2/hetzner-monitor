#!/usr/bin/env bash
# M3b smoke test, READ-ONLY: runs one inventory cycle (manual sync) against the real project and
# shows what the new inventory, hygiene and cost checks found, plus the estimated monthly cost
# to compare with the Hetzner Console. Nothing is changed in Hetzner or in the rules; a project
# this script connected is disconnected again on exit.
#
# Usage (secrets only via environment, never as arguments):
#   read -rs HM_API_KEY && export HM_API_KEY          # hetzner-monitor key (write scope, for sync)
#   read -rs HCLOUD_TOKEN && export HCLOUD_TOKEN      # READ-only Hetzner token
#   scripts/smoke-m3b.sh
#   HM_PROJECT_ID=<uuid> scripts/smoke-m3b.sh         # use an already connected project
# Python 3.9 compatible; nothing secret is printed.
set -euo pipefail

: "${HM_API_KEY:?export HM_API_KEY (hetzner-monitor API key with write scope)}"
HM_URL="${HM_URL:-http://127.0.0.1:8080}"

auth_header() { printf 'Authorization: Bearer %s' "$HM_API_KEY"; }
api() { # method path [curl args...] — secrets go via header file / stdin, not argv
  local method="$1" path="$2"; shift 2
  curl -sS -X "$method" -H @<(auth_header) -H 'Content-Type: application/json' "$@" "$HM_URL/api/v1$path"
}
json_field() { python3 -I -c 'import json,sys; v=json.loads(sys.argv[1]); [v := v[k] for k in sys.argv[2:]]; print(v)' "$@"; }

rules="$(api GET "/rules" -w '\n%{http_code}')"
status="${rules##*$'\n'}"; rules="${rules%$'\n'*}"
if [[ "$status" != "200" ]] || ! grep -q '"cost_change"' <<<"$rules"; then
  echo "✗ GET /rules returned HTTP $status or no cost_change rule — is the stack running M3b and HM_API_KEY valid?"
  echo "  rebuild: docker compose -f deploy/docker-compose.yml up --build -d"
  exit 1
fi

project_id="${HM_PROJECT_ID:-}"
connected_here=""
cleanup() {
  if [[ -n "$connected_here" ]]; then
    api DELETE "/projects/$project_id" -o /dev/null && echo "→ project disconnected again"
  fi
}
trap cleanup EXIT

if [[ -z "$project_id" ]]; then
  : "${HCLOUD_TOKEN:?export HCLOUD_TOKEN (read-only token) or set HM_PROJECT_ID}"
  export SMOKE_PROJECT="${SMOKE_PROJECT:-smoke-m3b-$(date +%Y%m%d-%H%M%S)}"
  echo "→ connecting project '$SMOKE_PROJECT' (read-only) at $HM_URL"
  response="$(python3 -I -c 'import json,os; print(json.dumps({"name": os.environ["SMOKE_PROJECT"], "token": os.environ["HCLOUD_TOKEN"]}))' \
    | api POST /projects --data-binary @- -w '\n%{http_code}')"
  status="${response##*$'\n'}"; body="${response%$'\n'*}"
  if [[ "$status" != "201" ]]; then
    echo "✗ connect failed: HTTP $status"; echo "$body"; exit 1
  fi
  project_id="$(json_field "$body" project id)"
  connected_here=1
  echo "✓ project $project_id connected (first inventory cycle done)"
else
  echo "→ manual sync of project $project_id (one inventory cycle, GET requests only)"
  for attempt in 1 2 3 4 5 6; do
    response="$(api POST "/projects/$project_id/sync" -w '\n%{http_code}')"
    status="${response##*$'\n'}"
    [[ "$status" != "409" ]] && break
    echo "  a poll cycle is running, retrying in 5 s ($attempt/6)"; sleep 5
  done
  if [[ "$status" != "200" ]]; then
    echo "✗ sync failed: HTTP $status"; echo "${response%$'\n'*}"; exit 1
  fi
  echo "✓ synced"
fi

project="$(api GET "/projects/$project_id")"
python3 -I -c '
import json, sys
p = json.loads(sys.argv[1])["project"]
c = p.get("cost_estimate")
print("")
print("Budget: {} requests/h allowed, fast + inventory tiers use {} (incl. /pricing)".format(p["budget_h"], p["fixed_h"]))
if c:
    print("Estimated monthly cost ({}): {:.2f} {} net".format(c["day"], c["monthly_net"], c["currency"]))
    print("  servers (+ backups), load balancers, volumes, primary/floating IPs; no traffic overage, snapshots, object storage")
    if c["unpriced"]:
        print("  {} resource(s) had no price in /pricing and are not included".format(c["unpriced"]))
    print("  → compare with the Hetzner Console (Billing / project costs, net)")
else:
    print("✗ no cost estimate — did the inventory cycle run?")
' "$project"

alerts="$(api GET "/alerts?project_id=$project_id&state=open&per_page=100")"
python3 -I -c '
import json, sys
checks = ["certificate_expiry", "deprecation", "hygiene_backups", "hygiene_protection",
          "hygiene_orphans", "cost_change", "traffic_quota"]
alerts = json.loads(sys.argv[1]).get("alerts", [])
print("")
print("Open alerts of the M3b checks (pending ones fire after the rule hold time: hygiene 1 h, orphans 1 day):")
for check in checks:
    found = [a for a in alerts if a["check_type"] == check]
    if check == "traffic_quota":
        found = [a for a in found if "forecast" in a["message"]]
        check = "traffic_quota (forecast)"
    print("  {:<26} {}".format(check, len(found) or "-"))
    for a in found:
        print("      {:<8} {:<8} {}".format(a["state"], a["severity"], a["message"]))
print("")
print("Expected on day one: no cost_change (needs an earlier day) and no forecast (needs a day-old traffic sample).")
' "$alerts"
echo "✓ smoke test done — please check the cost estimate and the findings for plausibility"
