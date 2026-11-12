#!/usr/bin/env bash
# M4 smoke test: verifies notification channels against the running stack. For every channel
# whose target is set in the environment (or deploy/.env), it creates a temporary channel,
# sends `POST /channels/{id}/test` and deletes the channel again. Hetzner is not touched.
#
# Targets (secrets: never as arguments, never in the chat; put them in deploy/.env or export them):
#   HM_TEST_SLACK_URL     Slack incoming webhook URL
#   HM_TEST_DISCORD_URL   Discord webhook URL
#   HM_TEST_WEBHOOK_URL   any https endpoint (e.g. a request bin); secret is generated
#   HM_TEST_NTFY_TOPIC    ntfy topic (HM_TEST_NTFY_SERVER default https://ntfy.sh, HM_TEST_NTFY_TOKEN optional)
#   HM_TEST_EMAIL_TO      recipient address (needs HM_SMTP_* on the server)
# Plus HM_API_KEY (write scope). Usage: scripts/smoke-m4.sh   — Python 3.9 compatible; nothing secret is printed.
set -euo pipefail

vars=(HM_API_KEY HM_TEST_SLACK_URL HM_TEST_DISCORD_URL HM_TEST_WEBHOOK_URL HM_TEST_NTFY_TOPIC
      HM_TEST_NTFY_SERVER HM_TEST_NTFY_TOKEN HM_TEST_EMAIL_TO)
env_file="${HM_ENV_FILE:-$(dirname "$0")/../deploy/.env}"
for var in "${vars[@]}"; do
  if [[ -z "${!var:-}" && -f "$env_file" ]]; then
    value="$(python3 -I -c '
import sys
name, path = sys.argv[1], sys.argv[2]
for line in open(path):
    line = line.strip()
    if line.startswith("export "):
        line = line[len("export "):]
    if line.startswith(name + "="):
        print(line.split("=", 1)[1].strip().strip("\"\x27"))
        break
' "$var" "$env_file")"
    if [[ -n "$value" ]]; then export "$var=$value"; fi
    unset value
  fi
done

: "${HM_API_KEY:?export HM_API_KEY (write scope) or put it in deploy/.env}"
HM_URL="${HM_URL:-http://127.0.0.1:8080}"

auth_header() { printf 'Authorization: Bearer %s' "$HM_API_KEY"; }
api() { # method path [curl args...] — the key goes via a header file, bodies via stdin
  local method="$1" path="$2"; shift 2
  curl -sS -X "$method" -H @<(auth_header) -H 'Content-Type: application/json' "$@" "$HM_URL/api/v1$path"
}

status="$(api GET /channels -o /dev/null -w '%{http_code}')"
if [[ "$status" != "200" ]]; then
  echo "✗ GET /channels returned HTTP $status — is the stack running M4 and HM_API_KEY valid?"
  echo "  rebuild: docker compose -f deploy/docker-compose.yml up --build -d"
  exit 1
fi

created=()
cleanup() {
  for id in "${created[@]:-}"; do
    [[ -n "$id" ]] && api DELETE "/channels/$id" -o /dev/null
  done
  if [[ ${#created[@]} -gt 0 ]]; then echo "→ temporary channels deleted"; fi
}
trap cleanup EXIT

# Builds the POST /channels body from the environment for one type (secrets read inside Python).
body_for() {
  python3 -I -c '
import json, os, secrets, sys
kind = sys.argv[1]
e = os.environ.get
if kind == "slack":
    cfg = {"url": e("HM_TEST_SLACK_URL")}
elif kind == "discord":
    cfg = {"url": e("HM_TEST_DISCORD_URL")}
elif kind == "webhook":
    cfg = {"url": e("HM_TEST_WEBHOOK_URL"), "secret": secrets.token_urlsafe(24)}
elif kind == "ntfy":
    cfg = {"topic": e("HM_TEST_NTFY_TOPIC"), "server": e("HM_TEST_NTFY_SERVER") or "https://ntfy.sh"}
    if e("HM_TEST_NTFY_TOKEN"):
        cfg["token"] = e("HM_TEST_NTFY_TOKEN")
else:
    cfg = {"to": [e("HM_TEST_EMAIL_TO")]}
print(json.dumps({"type": kind, "name": "smoke-m4-{}-{}".format(kind, secrets.token_hex(3)),
                  "config": cfg, "min_severity": "critical", "enabled": False}))
' "$1"
}

field() { python3 -I -c 'import json,sys; v=json.loads(sys.argv[1]); [v := v[k] for k in sys.argv[2:]]; print(v)' "$@"; }

tested=0; failed=0
for kind in slack discord webhook ntfy email; do
  case "$kind" in
    slack) target="${HM_TEST_SLACK_URL:-}" ;;
    discord) target="${HM_TEST_DISCORD_URL:-}" ;;
    webhook) target="${HM_TEST_WEBHOOK_URL:-}" ;;
    ntfy) target="${HM_TEST_NTFY_TOPIC:-}" ;;
    email) target="${HM_TEST_EMAIL_TO:-}" ;;
  esac
  if [[ -z "$target" ]]; then
    printf '  %-8s - (no target set)\n' "$kind"
    continue
  fi
  response="$(body_for "$kind" | api POST /channels --data-binary @- -w '\n%{http_code}')"
  status="${response##*$'\n'}"; body="${response%$'\n'*}"
  if [[ "$status" != "201" ]]; then
    printf '  %-8s ✗ create failed: HTTP %s %s\n' "$kind" "$status" "$(field "$body" error message 2>/dev/null || true)"
    failed=$((failed + 1)); continue
  fi
  id="$(field "$body" channel id)"; created+=("$id")
  hint="$(field "$body" channel hint)"
  result="$(api POST "/channels/$id/test")"
  tested=$((tested + 1))
  if [[ "$(field "$result" ok)" == "True" ]]; then
    printf '  %-8s ✓ test sent (%s) — please confirm it arrived\n' "$kind" "$hint"
  else
    printf '  %-8s ✗ %s (retryable: %s)\n' "$kind" "$(field "$result" error)" "$(field "$result" retryable)"
    failed=$((failed + 1))
  fi
done

echo ""
if [[ "$tested" -eq 0 && "$failed" -eq 0 ]]; then
  echo "✗ no channel target set — put e.g. HM_TEST_NTFY_TOPIC=<topic> into deploy/.env"
  exit 1
fi
echo "Channels tested: $tested, failed: $failed. The channels were created disabled and are deleted now."
[[ "$failed" -eq 0 ]]
