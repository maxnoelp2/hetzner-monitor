//! Rate-limit budget maths (docs/design/scheduler.md#budget-maths, ADR-0010). Pure functions.

/// Hetzner's documented limit, used until the first response tells us the real one.
pub const DEFAULT_LIMIT: u32 = 3600;
/// Items per page (Hetzner's maximum).
pub const PER_PAGE: u32 = 50;
/// Fast tier cadence.
pub const FAST_INTERVAL_S: u32 = 60;
/// Inventory tier cadence.
pub const INVENTORY_INTERVAL_S: u32 = 900;
/// Lower bound of the metrics interval.
pub const MIN_METRICS_INTERVAL_S: u32 = 60;
/// Upper bound of the error backoff.
pub const MAX_BACKOFF_S: u32 = 900;

/// Requests needed to list `n` items of one kind (an empty list still costs one request).
pub const fn list_requests(n: u32) -> u32 {
    if n == 0 { 1 } else { n.div_ceil(PER_PAGE) }
}

/// Requests per fast cycle: servers, plus load balancers if the project has any
/// (new load balancers are picked up by the inventory tier).
pub const fn fast_cost(servers: u32, load_balancers: u32) -> u32 {
    let lbs = if load_balancers == 0 {
        0
    } else {
        list_requests(load_balancers)
    };
    list_requests(servers) + lbs
}

/// Extra requests per inventory cycle besides the listings: `GET /pricing`.
pub const INVENTORY_EXTRA_REQUESTS: u32 = 1;

/// Requests per inventory cycle: one listing per kind, plus `GET /pricing`.
pub fn inventory_cost(counts_per_kind: &[u32]) -> u32 {
    counts_per_kind
        .iter()
        .map(|n| list_requests(*n))
        .sum::<u32>()
        + INVENTORY_EXTRA_REQUESTS
}

/// Requests per hour this tool may use: `limit × share`, rounded down.
pub fn budget_h(limit: Option<u32>, share: f64) -> u32 {
    let limit = f64::from(limit.unwrap_or(DEFAULT_LIMIT));
    let share = share.clamp(0.0, 1.0);
    // Both factors are bounded, so the product fits into u32.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let budget = (limit * share).floor() as u32;
    budget
}

/// Requests per hour spent by the fixed tiers (fast + inventory).
pub const fn fixed_h(fast_cost: u32, inventory_cost: u32) -> u32 {
    fast_cost * (3600 / FAST_INTERVAL_S) + inventory_cost * (3600 / INVENTORY_INTERVAL_S)
}

/// Metrics interval in seconds for `n_metrics` metric requests per cycle, or `None` when the
/// fixed tiers already use the whole budget (metrics paused) or there is nothing to fetch.
/// Doubles while Hetzner reports less than 20 % of the limit remaining.
pub fn metrics_interval_s(
    n_metrics: u32,
    budget_h: u32,
    fixed_h: u32,
    remaining: Option<u32>,
    limit: Option<u32>,
) -> Option<u32> {
    if n_metrics == 0 || budget_h <= fixed_h {
        return None;
    }
    let left = u64::from(budget_h - fixed_h);
    let needed = u64::from(n_metrics) * 3600;
    let interval = u32::try_from(needed.div_ceil(left))
        .unwrap_or(u32::MAX)
        .max(MIN_METRICS_INTERVAL_S);
    let low = match (remaining, limit) {
        (Some(remaining), Some(limit)) => u64::from(remaining) * 5 < u64::from(limit),
        _ => false,
    };
    Some(if low {
        interval.saturating_mul(2)
    } else {
        interval
    })
}

/// Wait before the next attempt after `failures` consecutive failures (1 → 60 s, doubling, ≤ 15 min).
pub fn backoff_s(failures: u32) -> u32 {
    let exp = failures.saturating_sub(1).min(10);
    FAST_INTERVAL_S.saturating_mul(1 << exp).min(MAX_BACKOFF_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_example_100_servers_gives_220_s() {
        // docs/design/scheduler.md: budget 1800/h, fast 120/h, inventory ≈ 40/h → 220 s.
        assert_eq!(budget_h(Some(3600), 0.5), 1800);
        assert_eq!(metrics_interval_s(100, 1800, 160, None, None), Some(220));
    }

    #[test]
    fn costs_count_pages() {
        assert_eq!(list_requests(0), 1);
        assert_eq!(list_requests(50), 1);
        assert_eq!(list_requests(51), 2);
        assert_eq!(fast_cost(100, 0), 2);
        assert_eq!(fast_cost(9, 1), 2);
        assert_eq!(inventory_cost(&[9, 0, 0, 0, 0, 16]), 7);
        assert_eq!(fixed_h(2, 10), 2 * 60 + 10 * 4);
    }

    #[test]
    fn small_projects_hit_the_lower_bound() {
        assert_eq!(metrics_interval_s(9, 1800, 84, None, None), Some(60));
    }

    #[test]
    fn exhausted_budget_pauses_metrics() {
        assert_eq!(metrics_interval_s(10, 100, 100, None, None), None);
        assert_eq!(metrics_interval_s(0, 1800, 100, None, None), None);
    }

    #[test]
    fn low_remaining_doubles_the_interval() {
        assert_eq!(
            metrics_interval_s(100, 1800, 160, Some(700), Some(3600)),
            Some(440)
        );
        assert_eq!(
            metrics_interval_s(100, 1800, 160, Some(720), Some(3600)),
            Some(220)
        );
    }

    #[test]
    fn budget_share_and_missing_limit() {
        assert_eq!(budget_h(None, 0.5), 1800);
        assert_eq!(budget_h(Some(3600), 0.1), 360);
    }

    #[test]
    fn backoff_doubles_up_to_15_min() {
        assert_eq!(backoff_s(1), 60);
        assert_eq!(backoff_s(2), 120);
        assert_eq!(backoff_s(4), 480);
        assert_eq!(backoff_s(5), 900);
        assert_eq!(backoff_s(50), 900);
    }
}
