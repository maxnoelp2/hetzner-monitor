//! Estimated monthly cost of a project (docs/design/checks.md#cost-estimate). Pure: prices come
//! from the `pricing` object of `GET /pricing`, quantities from the inventory snapshots.
//! Field shapes follow the Hetzner `OpenAPI` spec (verified 2026-11-12).
//!
//! The estimate is the **net** monthly price of everything in the inventory: server types
//! (plus the backup surcharge when backups are on), load balancer types, volumes by size,
//! primary and floating IPs. Hourly billing is capped at the monthly price, so this is the
//! "full month" figure. Not included: traffic overage, snapshots/images, object storage.

use serde_json::Value;

/// Result of [`estimate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    /// Net monthly cost in `currency`, rounded to cents.
    pub monthly_net: f64,
    /// Hetzner's currency for the account (e.g. `EUR`).
    pub currency: String,
    /// Resources that cost money but had no price (labels like `server web-1 (1)`).
    pub unpriced: Vec<String>,
}

/// Estimates the monthly cost of `resources` (`(kind, snapshot)` as stored in `resources`).
/// Certificates are free and ignored.
pub fn estimate<'a>(
    pricing: &Value,
    resources: impl IntoIterator<Item = (&'a str, &'a Value)>,
) -> Estimate {
    let mut total = 0.0;
    let mut unpriced = Vec::new();
    for (kind, snapshot) in resources {
        let price = match kind {
            "server" => server_price(pricing, snapshot),
            "load_balancer" => typed_price(
                pricing,
                snapshot,
                "load_balancer_types",
                "load_balancer_type",
            ),
            "volume" => volume_price(pricing, snapshot),
            "primary_ip" => ip_price(pricing, snapshot, "primary_ips", "/location/name")
                .or_else(|| free_primary_ipv6(pricing, snapshot)),
            "floating_ip" => ip_price(pricing, snapshot, "floating_ips", "/home_location/name")
                .or_else(|| net(pricing.pointer("/floating_ip/price_monthly"))),
            _ => continue,
        };
        match price {
            Some(p) => total += p,
            None => unpriced.push(label(kind, snapshot)),
        }
    }
    Estimate {
        monthly_net: (total * 100.0).round() / 100.0,
        currency: pricing
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("EUR")
            .to_owned(),
        unpriced,
    }
}

/// `{"net": "1.23", …}` → 1.23 (Hetzner sends decimal strings).
fn net(price: Option<&Value>) -> Option<f64> {
    let v = price?.get("net")?;
    let n = match v {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        other => other.as_f64()?,
    };
    (n.is_finite() && n >= 0.0).then_some(n)
}

fn decimal(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::String(s) => s.trim().parse::<f64>().ok(),
        other => other.as_f64(),
    }
    .filter(|n| n.is_finite() && *n >= 0.0)
}

/// The monthly net price for `location` in a `prices[]` array.
fn monthly_at(prices: Option<&Value>, location: &str) -> Option<f64> {
    prices?
        .as_array()?
        .iter()
        .find(|p| p.get("location").and_then(Value::as_str) == Some(location))
        .and_then(|p| net(p.get("price_monthly")))
}

/// Server type or load balancer type price: `/pricing` by type name and location, falling
/// back to the `prices` embedded in the snapshot's type object.
fn typed_price(pricing: &Value, snapshot: &Value, collection: &str, field: &str) -> Option<f64> {
    let location = snapshot.pointer("/location/name").and_then(Value::as_str)?;
    let type_obj = snapshot.get(field)?;
    let name = type_obj.get("name").and_then(Value::as_str);
    pricing
        .get(collection)
        .and_then(Value::as_array)
        .and_then(|types| {
            types
                .iter()
                .find(|t| name.is_some() && t.get("name").and_then(Value::as_str) == name)
        })
        .and_then(|t| monthly_at(t.get("prices"), location))
        .or_else(|| monthly_at(type_obj.get("prices"), location))
}

fn server_price(pricing: &Value, snapshot: &Value) -> Option<f64> {
    let base = typed_price(pricing, snapshot, "server_types", "server_type")?;
    let backups = snapshot.get("backup_window").is_some_and(|w| !w.is_null());
    if !backups {
        return Some(base);
    }
    let pct = decimal(pricing.pointer("/server_backup/percentage"))?;
    Some(base * (1.0 + pct / 100.0))
}

fn volume_price(pricing: &Value, snapshot: &Value) -> Option<f64> {
    let size = snapshot.get("size").and_then(Value::as_f64)?;
    let per_gb = net(pricing.pointer("/volume/price_per_gb_month"))?;
    Some(size * per_gb)
}

fn ip_price(pricing: &Value, snapshot: &Value, collection: &str, location: &str) -> Option<f64> {
    let ip_type = snapshot.get("type").and_then(Value::as_str)?;
    let location = snapshot.pointer(location).and_then(Value::as_str)?;
    let entry = pricing
        .get(collection)?
        .as_array()?
        .iter()
        .find(|t| t.get("type").and_then(Value::as_str) == Some(ip_type))?;
    monthly_at(entry.get("prices"), location)
}

/// Primary IPv6 is free at Hetzner, and the real `/pricing` lists only `ipv4` under
/// `primary_ips` (seen 2026-11-12). An IPv6 primary IP counts as 0 when `/pricing` has no
/// `ipv6` entry at all; an entry without the IP's location still leaves it unpriced.
fn free_primary_ipv6(pricing: &Value, snapshot: &Value) -> Option<f64> {
    if snapshot.get("type").and_then(Value::as_str) != Some("ipv6") {
        return None;
    }
    let listed = pricing
        .get("primary_ips")
        .and_then(Value::as_array)
        .is_some_and(|e| {
            e.iter()
                .any(|t| t.get("type").and_then(Value::as_str) == Some("ipv6"))
        });
    (!listed).then_some(0.0)
}

fn label(kind: &str, snapshot: &Value) -> String {
    let id = snapshot
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let what = kind.replace('_', " ");
    match snapshot.get("name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() => format!("{what} {name} ({id})"),
        _ => format!("{what} {id}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Shaped after the spec's `GET /pricing` example; values made up.
    fn pricing() -> Value {
        json!({
            "currency": "EUR", "vat_rate": "19.00",
            "volume": { "price_per_gb_month": { "net": "0.0440000000", "gross": "0.0523600000000000" } },
            "server_backup": { "percentage": "20.00" },
            "server_types": [{ "id": 1, "name": "cx22", "prices": [
                { "location": "fsn1", "price_monthly": { "net": "4.0000000000", "gross": "4.7600000000000000" } },
                { "location": "ash", "price_monthly": { "net": "5.0000000000", "gross": "5.0000000000000000" } }
            ]}],
            "load_balancer_types": [{ "id": 1, "name": "lb11", "prices": [
                { "location": "fsn1", "price_monthly": { "net": "5.3900000000", "gross": "6.4141000000000000" } }
            ]}],
            "primary_ips": [
                { "type": "ipv4", "prices": [{ "location": "fsn1", "price_monthly": { "net": "0.5000000000", "gross": "0.5950000000000000" } }] },
                { "type": "ipv6", "prices": [{ "location": "fsn1", "price_monthly": { "net": "0.0000000000", "gross": "0.0000000000000000" } }] }
            ],
            "floating_ips": [
                { "type": "ipv4", "prices": [{ "location": "fsn1", "price_monthly": { "net": "3.0000000000", "gross": "3.5700000000000000" } }] }
            ]
        })
    }

    fn server(id: i64, location: &str, backups: bool) -> Value {
        json!({ "id": id, "name": format!("web-{id}"), "location": { "name": location },
                "server_type": { "name": "cx22" }, "backup_window": if backups { json!("22-02") } else { Value::Null } })
    }

    #[test]
    fn sums_every_priced_kind() {
        let resources = [
            ("server", server(1, "fsn1", false)),
            ("server", server(2, "ash", true)), // 5.00 + 20 % backups
            (
                "load_balancer",
                json!({ "id": 3, "name": "lb", "location": { "name": "fsn1" }, "load_balancer_type": { "name": "lb11" } }),
            ),
            (
                "volume",
                json!({ "id": 4, "name": "data", "size": 100, "location": { "name": "fsn1" } }),
            ),
            (
                "primary_ip",
                json!({ "id": 5, "type": "ipv4", "location": { "name": "fsn1" } }),
            ),
            (
                "primary_ip",
                json!({ "id": 6, "type": "ipv6", "location": { "name": "fsn1" } }),
            ),
            (
                "floating_ip",
                json!({ "id": 7, "type": "ipv4", "home_location": { "name": "fsn1" } }),
            ),
            ("certificate", json!({ "id": 8, "name": "cert" })),
        ];
        let est = estimate(&pricing(), resources.iter().map(|(k, v)| (*k, v)));
        // 4 + 6 + 5.39 + 4.4 + 0.5 + 0 + 3 = 23.29
        assert!(
            (est.monthly_net - 23.29).abs() < 1e-9,
            "{}",
            est.monthly_net
        );
        assert_eq!(est.currency, "EUR");
        assert_eq!(est.unpriced, Vec::<String>::new());
    }

    #[test]
    fn unknown_prices_are_reported_not_guessed() {
        let resources = [
            ("server", server(1, "hel1", false)),
            (
                "primary_ip",
                json!({ "id": 5, "name": "", "type": "ipv4", "location": { "name": "nbg1" } }),
            ),
        ];
        let est = estimate(&pricing(), resources.iter().map(|(k, v)| (*k, v)));
        assert!(est.monthly_net.abs() < f64::EPSILON);
        assert_eq!(est.unpriced, ["server web-1 (1)", "primary ip 5"]);
    }

    #[test]
    fn primary_ipv6_is_free_when_pricing_does_not_list_it() {
        // The real `/pricing` (2026-11-12) has only `ipv4` under `primary_ips`.
        let mut pricing = pricing();
        pricing["primary_ips"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["type"] == "ipv4");
        let v6 = json!({ "id": 6, "type": "ipv6", "location": { "name": "nbg1" } });
        let est = estimate(&pricing, [("primary_ip", &v6)]);
        assert!(est.monthly_net.abs() < f64::EPSILON);
        assert_eq!(est.unpriced, Vec::<String>::new());
        // Listed, but not for this location: still unpriced, not guessed.
        let est = estimate(&self::pricing(), [("primary_ip", &v6)]);
        assert_eq!(est.unpriced, ["primary ip 6"]);
    }

    #[test]
    fn falls_back_to_the_prices_embedded_in_the_snapshot() {
        let snap = json!({ "id": 9, "name": "new", "location": { "name": "nbg1" }, "backup_window": null,
            "server_type": { "name": "cx99", "prices": [{ "location": "nbg1", "price_monthly": { "net": "12.5", "gross": "14.875" } }] } });
        let est = estimate(&pricing(), [("server", &snap)]);
        assert!((est.monthly_net - 12.5).abs() < 1e-9);
    }
}
