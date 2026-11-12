#!/usr/bin/env bash
# M5a smoke test against the running local stack: password login → GET /me → a state-changing
# request is refused without the CSRF header and accepted with it → logout → the session is gone.
# Hetzner is not touched. The only write is a silence whose selector matches nothing, deleted at once.
#
# Needs HM_EMAIL and HM_PASSWORD (from the environment or deploy/.env; never as arguments, never in
# the chat). Get a password with:
#   docker compose -f deploy/docker-compose.yml run --rm app users reset-password --email <email>
# Usage: scripts/smoke-m5a.sh   — Python 3.9 compatible; nothing secret is printed.
set -euo pipefail

env_file="${HM_ENV_FILE:-$(dirname "$0")/../deploy/.env}"
for var in HM_EMAIL HM_PASSWORD; do
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
: "${HM_EMAIL:?export HM_EMAIL or put it in deploy/.env}"
: "${HM_PASSWORD:?export HM_PASSWORD or put it in deploy/.env}"
HM_URL="${HM_URL:-http://127.0.0.1:8080}"

work="$(mktemp -d)"
chmod 700 "$work"
trap 'rm -rf "$work"' EXIT

fail() { echo "✗ $*"; exit 1; }
field() { # file key.path — prints a JSON field (no secrets are passed to it)
  python3 -I -c '
import json, sys
v = json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."):
    v = v.get(k) if isinstance(v, dict) else None
print("" if v is None else v)
' "$1" "$2"
}

# 1. Login: the body is built from the environment and sent via stdin.
python3 -I -c 'import json, os; print(json.dumps({"email": os.environ["HM_EMAIL"], "password": os.environ["HM_PASSWORD"]}))' \
  > "$work/login.json"
status="$(curl -sS -o "$work/me.json" -D "$work/headers" -w '%{http_code}' -X POST \
  -H 'Content-Type: application/json' --data-binary @"$work/login.json" "$HM_URL/api/v1/auth/login")"
rm -f "$work/login.json"
[[ "$status" == "200" ]] || fail "login returned HTTP $status ($(field "$work/me.json" error.code)) — is the stack running M5a and are HM_EMAIL/HM_PASSWORD right?"
python3 -I -c '
import sys
for line in open(sys.argv[1]):
    if line.lower().startswith("set-cookie:") and "hm_session=" in line:
        cookie = line.split(":", 1)[1].strip().split(";")[0]
        flags = [f.strip() for f in line.split(";")[1:]]
        open(sys.argv[2], "w").write("Cookie: " + cookie + "\n")
        print("  cookie flags: " + ", ".join(f for f in flags if f))
        break
else:
    sys.exit("no hm_session cookie in the login response")
' "$work/headers" "$work/cookie"
printf 'X-CSRF-Token: %s\n' "$(field "$work/me.json" csrf_token)" > "$work/csrf"
echo "✓ login as $(field "$work/me.json" user.email) (role $(field "$work/me.json" role), org $(field "$work/me.json" org.name))"

api() { # method path [curl args...] — cookie and CSRF header come from files
  local method="$1" path="$2"; shift 2
  curl -sS -X "$method" -H @"$work/cookie" -H 'Content-Type: application/json' "$@" "$HM_URL/api/v1$path"
}

# 2. /me with the cookie.
[[ "$(api GET /me -o "$work/me2.json" -w '%{http_code}')" == "200" ]] || fail "GET /me with the session failed"
[[ "$(field "$work/me2.json" auth)" == "session" ]] || fail "GET /me did not report a session"
echo "✓ GET /me with the session cookie"

# 3. CSRF: refused without the header, accepted with it.
body='{"ends_at":"'"$(python3 -I -c 'import datetime; print((datetime.datetime.utcnow() + datetime.timedelta(minutes=5)).strftime("%Y-%m-%dT%H:%M:%SZ"))')"'","selector":"hm-smoke=none","reason":"smoke-m5a"}'
status="$(api POST /silences -o "$work/out.json" -w '%{http_code}' --data-binary "$body")"
[[ "$status" == "403" && "$(field "$work/out.json" error.code)" == "csrf_failed" ]] || fail "POST without CSRF header returned HTTP $status"
echo "✓ POST without X-CSRF-Token → 403 csrf_failed"
status="$(api POST /silences -H @"$work/csrf" -o "$work/out.json" -w '%{http_code}' --data-binary "$body")"
[[ "$status" == "201" ]] || fail "POST with CSRF header returned HTTP $status"
silence="$(field "$work/out.json" silence.id)"
[[ "$(api DELETE "/silences/$silence" -H @"$work/csrf" -o /dev/null -w '%{http_code}')" == "204" ]] || fail "could not delete the test silence $silence"
echo "✓ POST with X-CSRF-Token → 201 (test silence deleted again)"

# 4. Logout ends the session.
[[ "$(api POST /auth/logout -H @"$work/csrf" -o /dev/null -w '%{http_code}')" == "204" ]] || fail "logout failed"
[[ "$(api GET /me -o /dev/null -w '%{http_code}')" == "401" ]] || fail "session still valid after logout"
echo "✓ logout → the session cookie no longer works"
echo "Done."
