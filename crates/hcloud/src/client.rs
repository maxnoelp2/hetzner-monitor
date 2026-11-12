use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use serde_json::{Map, Value};

use crate::error::Error;
use std::collections::BTreeMap;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::model::{
    Listed, MetricType, Metrics, Priced, RateLimit, RawResource, ResourceKind, Token,
};

/// Production API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.hetzner.cloud/v1";

const PER_PAGE: u32 = 50;
/// Safety stop for pagination (50 × 1000 = 50 000 items of one kind).
const MAX_PAGES: u32 = 1000;

/// Read-only Hetzner Cloud API client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    /// Client for the production API.
    pub fn new() -> Result<Self, Error> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    /// Client for another base URL (tests).
    pub fn with_base_url(base_url: &str) -> Result<Self, Error> {
        // reqwest is built without a bundled crypto provider; share ring with sqlx.
        // Installing fails harmlessly if a provider is already installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("hetzner-monitor/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|err| Error::Setup(err.without_url().to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }

    /// Checks that the token works with one cheap request (`GET /servers?per_page=1`).
    pub async fn validate_token(&self, token: &Token) -> Result<RateLimit, Error> {
        let (_, rate_limit) = self.get_json(token, "servers?per_page=1").await?;
        Ok(rate_limit)
    }

    /// Lists all resources of `kind`, following pagination.
    pub async fn list(
        &self,
        token: &Token,
        kind: ResourceKind,
    ) -> Result<Listed<RawResource>, Error> {
        let collection = kind.collection();
        let mut items = Vec::new();
        let mut rate_limit;
        let mut page = 1;
        loop {
            let (body, rl) = self
                .get_json(
                    token,
                    &format!("{collection}?per_page={PER_PAGE}&page={page}"),
                )
                .await?;
            rate_limit = rl;
            let array = body
                .get(collection)
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Decode(format!("missing `{collection}` array")))?;
            for item in array {
                items.push(raw_resource(item)?);
            }
            match next_page(&body) {
                Some(next) if next > page && next <= MAX_PAGES => page = next,
                Some(_) => return Err(Error::Decode("invalid pagination".to_owned())),
                None => break,
            }
        }
        Ok(Listed { items, rate_limit })
    }

    /// Metric series of one server between `start` and `end` (UTC), at about `step_s` seconds
    /// resolution (Hetzner may coarsen it to stay within 500 samples).
    pub async fn server_metrics(
        &self,
        token: &Token,
        server_id: i64,
        types: &[MetricType],
        start: OffsetDateTime,
        end: OffsetDateTime,
        step_s: u32,
    ) -> Result<Metrics, Error> {
        if types.is_empty() {
            return Err(Error::Decode("no metric type requested".to_owned()));
        }
        let format = |t: OffsetDateTime| {
            t.to_offset(time::UtcOffset::UTC)
                .format(&Rfc3339)
                .map_err(|e| Error::Decode(e.to_string()))
        };
        let kinds: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
        let query = format!(
            "servers/{server_id}/metrics?type={}&start={}&end={}&step={step_s}",
            kinds.join(","),
            format(start)?,
            format(end)?
        );
        let (body, rate_limit) = self.get_json(token, &query).await?;
        parse_metrics(&body, rate_limit)
    }

    /// Current prices of the project's account (`GET /pricing`, not paginated). Returned as
    /// raw JSON: interpreting prices is domain logic (`hm_core::cost`).
    pub async fn pricing(&self, token: &Token) -> Result<Priced, Error> {
        let (body, rate_limit) = self.get_json(token, "pricing").await?;
        let raw = body
            .get("pricing")
            .filter(|p| p.is_object())
            .cloned()
            .ok_or_else(|| Error::Decode("missing `pricing` object".to_owned()))?;
        Ok(Priced { raw, rate_limit })
    }

    /// The only request method of this crate: authenticated `GET`.
    async fn get_json(
        &self,
        token: &Token,
        path_and_query: &str,
    ) -> Result<(Value, RateLimit), Error> {
        let url = format!("{}/{path_and_query}", self.base_url);
        let response = self
            .http
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {}", token.expose()))
            .send()
            .await
            .map_err(|err| Error::Unavailable {
                status: None,
                message: err.without_url().to_string(),
            })?;
        let status = response.status();
        let rate_limit = rate_limit(response.headers());
        let bytes = response.bytes().await.map_err(|err| Error::Unavailable {
            status: Some(status.as_u16()),
            message: err.without_url().to_string(),
        })?;
        if !status.is_success() {
            return Err(map_error(status, &bytes, rate_limit));
        }
        let body = serde_json::from_slice(&bytes).map_err(|err| Error::Decode(err.to_string()))?;
        Ok((body, rate_limit))
    }
}

fn rate_limit(headers: &HeaderMap) -> RateLimit {
    let parse = |name: &str| headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok();
    RateLimit {
        limit: parse("ratelimit-limit").and_then(|v| u32::try_from(v).ok()),
        remaining: parse("ratelimit-remaining").and_then(|v| u32::try_from(v).ok()),
        reset_at: parse("ratelimit-reset"),
    }
}

fn map_error(status: StatusCode, body: &[u8], rate_limit: RateLimit) -> Error {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let field = |name: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.pointer(&format!("/error/{name}")))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (code, message) = (field("code"), field("message"));
    match status.as_u16() {
        401 => Error::InvalidToken,
        403 if code == "maintenance" => Error::Maintenance { message },
        403 => Error::Forbidden { message },
        429 => Error::RateLimited {
            reset_at: rate_limit.reset_at,
        },
        500..=599 => Error::Unavailable {
            status: Some(status.as_u16()),
            message,
        },
        other => Error::Api {
            status: other,
            code,
            message,
        },
    }
}

fn next_page(body: &Value) -> Option<u32> {
    body.pointer("/meta/pagination/next_page")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
}

fn raw_resource(item: &Value) -> Result<RawResource, Error> {
    let id = item
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Decode("item without numeric `id`".to_owned()))?;
    Ok(RawResource {
        id,
        name: item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        labels: item
            .get("labels")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(Map::new),
        raw: item.clone(),
    })
}

fn parse_metrics(body: &Value, rate_limit: RateLimit) -> Result<Metrics, Error> {
    let metrics = body
        .get("metrics")
        .ok_or_else(|| Error::Decode("missing `metrics` object".to_owned()))?;
    let step = metrics.get("step").and_then(Value::as_f64).unwrap_or(60.0);
    let mut series = BTreeMap::new();
    if let Some(all) = metrics.get("time_series").and_then(Value::as_object) {
        for (name, entry) in all {
            let mut samples: Vec<(f64, f64)> = entry
                .get("values")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(sample).collect())
                .unwrap_or_default();
            samples.sort_by(|a, b| a.0.total_cmp(&b.0));
            series.insert(name.clone(), samples);
        }
    }
    Ok(Metrics {
        step,
        series,
        rate_limit,
    })
}

/// `[timestamp, "value"]` → `(ts, value)`; anything else (or a non-finite value) is dropped.
fn sample(value: &Value) -> Option<(f64, f64)> {
    let pair = value.as_array()?;
    let ts = pair.first()?.as_f64()?;
    let raw = pair.get(1)?;
    let v = match raw {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        other => other.as_f64()?,
    };
    (ts.is_finite() && v.is_finite()).then_some((ts, v))
}
