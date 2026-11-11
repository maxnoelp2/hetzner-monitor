//! Hetzner label selectors (`env=prod,role!=db`, `env in (a,b)`, `!k`), as documented in the
//! Hetzner Cloud API reference ("Label Selector"). Used to scope rules to resources.
//!
//! Semantics follow Kubernetes, which Hetzner's syntax is modelled on: `k!=v` and `k notin (…)`
//! also match resources without the key `k`.

use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};

/// A parsed selector: all requirements must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector(Vec<Requirement>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Requirement {
    Equals(String, String),
    NotEquals(String, String),
    Exists(String),
    NotExists(String),
    In(String, Vec<String>),
    NotIn(String, Vec<String>),
}

/// Why a selector could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid label selector: {0}")]
pub struct SelectorError(String);

impl Selector {
    /// True if `labels` satisfy every requirement. Non-string label values never match a value.
    pub fn matches(&self, labels: &Map<String, Value>) -> bool {
        let get = |key: &str| labels.get(key).and_then(Value::as_str);
        self.0.iter().all(|req| match req {
            Requirement::Equals(k, v) => get(k) == Some(v.as_str()),
            Requirement::NotEquals(k, v) => get(k) != Some(v.as_str()),
            Requirement::Exists(k) => labels.contains_key(k),
            Requirement::NotExists(k) => !labels.contains_key(k),
            Requirement::In(k, vs) => get(k).is_some_and(|v| vs.iter().any(|x| x == v)),
            Requirement::NotIn(k, vs) => !get(k).is_some_and(|v| vs.iter().any(|x| x == v)),
        })
    }
}

impl FromStr for Selector {
    type Err = SelectorError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let parts = split_top_level(input)?;
        let mut requirements = Vec::with_capacity(parts.len());
        for part in parts {
            requirements.push(requirement(part.trim())?);
        }
        Ok(Self(requirements))
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|req| match req {
                Requirement::Equals(k, v) => format!("{k}={v}"),
                Requirement::NotEquals(k, v) => format!("{k}!={v}"),
                Requirement::Exists(k) => k.clone(),
                Requirement::NotExists(k) => format!("!{k}"),
                Requirement::In(k, vs) => format!("{k} in ({})", vs.join(",")),
                Requirement::NotIn(k, vs) => format!("{k} notin ({})", vs.join(",")),
            })
            .collect();
        f.write_str(&parts.join(","))
    }
}

/// Splits on commas outside parentheses.
fn split_top_level(input: &str) -> Result<Vec<&str>, SelectorError> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0_u32, 0);
    for (i, c) in input.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| SelectorError("unbalanced `)`".to_owned()))?;
            }
            ',' if depth == 0 => {
                parts.push(&input[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(SelectorError("unbalanced `(`".to_owned()));
    }
    parts.push(&input[start..]);
    Ok(parts)
}

fn requirement(part: &str) -> Result<Requirement, SelectorError> {
    if part.is_empty() {
        return Err(SelectorError("empty requirement".to_owned()));
    }
    if let Some(open) = part.find('(') {
        let head: Vec<&str> = part[..open].split_whitespace().collect();
        let body = part[open + 1..]
            .strip_suffix(')')
            .ok_or_else(|| SelectorError(format!("expected `)` at the end of `{part}`")))?;
        let values = body
            .split(',')
            .map(|v| value(v.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        return match head.as_slice() {
            [k, "in"] => Ok(Requirement::In(key(k)?, values)),
            [k, "notin"] => Ok(Requirement::NotIn(key(k)?, values)),
            _ => Err(SelectorError(format!(
                "expected `key in (…)` or `key notin (…)` in `{part}`"
            ))),
        };
    }
    if let Some((k, v)) = part.split_once("!=") {
        return Ok(Requirement::NotEquals(key(k)?, value(v.trim())?));
    }
    if let Some((k, v)) = part.split_once("==").or_else(|| part.split_once('=')) {
        return Ok(Requirement::Equals(key(k)?, value(v.trim())?));
    }
    if let Some(k) = part.strip_prefix('!') {
        return Ok(Requirement::NotExists(key(k)?));
    }
    Ok(Requirement::Exists(key(part)?))
}

fn key(raw: &str) -> Result<String, SelectorError> {
    let k = raw.trim();
    let valid = !k.is_empty()
        && k.len() <= 317 // 253 prefix + `/` + 63 name
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if valid {
        Ok(k.to_owned())
    } else {
        Err(SelectorError(format!("invalid key `{k}`")))
    }
}

fn value(v: &str) -> Result<String, SelectorError> {
    let valid = v.len() <= 63
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(v.to_owned())
    } else {
        Err(SelectorError(format!("invalid value `{v}`")))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn labels(v: &Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn sel(s: &str) -> Selector {
        s.parse().unwrap()
    }

    #[test]
    fn equality_and_inequality() {
        let prod_web = labels(&json!({ "env": "prod", "role": "web" }));
        let prod_db = labels(&json!({ "env": "prod", "role": "db" }));
        let bare = labels(&json!({}));
        let s = sel("env=prod,role!=db");
        assert!(s.matches(&prod_web));
        assert!(!s.matches(&prod_db));
        assert!(!s.matches(&bare));
        assert!(sel("env==prod").matches(&prod_web));
        // `!=` matches a missing key.
        assert!(sel("role!=db").matches(&bare));
    }

    #[test]
    fn existence() {
        let l = labels(&json!({ "env": "prod" }));
        assert!(sel("env").matches(&l));
        assert!(!sel("!env").matches(&l));
        assert!(sel("!type").matches(&l));
        assert!(sel("env,!type").matches(&l));
    }

    #[test]
    fn set_based() {
        let staging = labels(&json!({ "env": "staging" }));
        let bare = labels(&json!({}));
        let s = sel("env in (testing, staging)");
        assert!(s.matches(&staging));
        assert!(!s.matches(&bare));
        assert!(!sel("env notin (staging)").matches(&staging));
        assert!(sel("env notin (staging)").matches(&bare));
        assert!(sel("env in (staging),!type").matches(&staging));
    }

    #[test]
    fn empty_value_matches_empty_label() {
        let l = labels(&json!({ "flag": "" }));
        assert!(sel("flag=").matches(&l));
    }

    #[test]
    fn non_string_values_never_match_a_value() {
        let l = labels(&json!({ "n": 1 }));
        assert!(!sel("n=1").matches(&l));
        assert!(sel("n").matches(&l));
    }

    #[test]
    fn invalid_selectors_are_rejected() {
        for bad in [
            "",
            "env=prod,",
            "env in (a",
            "env in a)",
            "env maybe (a)",
            "=prod",
            "env=pr od",
            "en v=prod",
            "!",
        ] {
            assert!(
                bad.parse::<Selector>().is_err(),
                "{bad:?} should be invalid"
            );
        }
    }

    #[test]
    fn display_round_trips() {
        let s = sel("env==prod, role!=db,env in (a,b),!x,y");
        assert_eq!(s.to_string(), "env=prod,role!=db,env in (a,b),!x,y");
        assert_eq!(sel(&s.to_string()), s);
    }
}
