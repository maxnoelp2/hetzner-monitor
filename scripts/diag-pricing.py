#!/usr/bin/env python3
# One-off READ-ONLY diagnosis for the M3b cost estimate: which resources the estimate could not
# price, and why. Three GET requests to api.hetzner.cloud (pricing, servers, primary_ips,
# floating_ips — four). Prints only types, locations and which fields exist; no names, IPs or
# secrets. Token from HCLOUD_TOKEN or deploy/.env. Python 3.9 compatible.
import json
import os
import sys
import urllib.request

here = os.path.dirname(os.path.abspath(__file__))
token = os.environ.get("HCLOUD_TOKEN", "")
if not token:
    for line in open(os.path.join(here, "..", "deploy", ".env")):
        line = line.strip()
        if line.startswith("export "):
            line = line[len("export "):]
        if line.startswith("HCLOUD_TOKEN="):
            token = line.split("=", 1)[1].strip().strip("\"'")
if not token:
    sys.exit("HCLOUD_TOKEN not set and not in deploy/.env")


def get(path):
    req = urllib.request.Request("https://api.hetzner.cloud/v1/" + path,
                                 headers={"Authorization": "Bearer " + token})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


pricing = get("pricing")["pricing"]
priced_types = {t["name"]: sorted(p["location"] for p in t.get("prices", [])) for t in pricing.get("server_types", [])}
print("pricing keys:", sorted(pricing))
for k in ("primary_ips", "floating_ips"):
    print("pricing.{}:".format(k), [(e.get("type"), sorted(p.get("location") for p in e.get("prices", []))) for e in pricing.get(k, [])])
print("pricing.floating_ip present:", "floating_ip" in pricing)


def loc(obj):
    return {
        "location": (obj.get("location") or {}).get("name"),
        "datacenter.location": ((obj.get("datacenter") or {}).get("location") or {}).get("name"),
    }


print("")
print("servers:")
for s in get("servers?per_page=50")["servers"]:
    st = s.get("server_type") or {}
    l = loc(s)
    where = l["location"] or l["datacenter.location"]
    in_pricing = st.get("name") in priced_types
    print("  type={:<7} {} type_in_/pricing={} loc_in_/pricing={} loc_in_embedded={} backups={}".format(
        str(st.get("name")), l, in_pricing,
        in_pricing and where in priced_types[st.get("name")],
        where in [p.get("location") for p in st.get("prices", [])],
        s.get("backup_window") is not None))

print("")
print("primary_ips:")
for ip in get("primary_ips?per_page=50")["primary_ips"]:
    print("  type={} {} assigned={}".format(ip.get("type"), loc(ip), ip.get("assignee_id") is not None))

print("")
print("floating_ips:")
for ip in get("floating_ips?per_page=50")["floating_ips"]:
    print("  type={} home_location={}".format(ip.get("type"), (ip.get("home_location") or {}).get("name")))
