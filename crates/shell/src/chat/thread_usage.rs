//! The "this thread" line in the chat's status row: what the thread has used so far.

use vivianne_ai::inference::usage_store::DayUsage;

/// "this thread: 12.4K tokens, $0.03", or `None` before the thread has used anything. A cost that is not known
/// says so: it is never shown as $0.
pub fn line(usage: &DayUsage) -> Option<String> {
    if usage.requests == 0 {
        return None;
    }
    let cost = match (usage.has_unknown_cost, usage.cost > 0.0) {
        (false, _) => money(usage.cost),
        (true, true) => format!("{}+ (some calls unpriced)", money(usage.cost)),
        (true, false) => "cost unknown".to_owned(),
    };
    Some(format!("this thread: {} tokens, {cost}", tokens(usage.tokens())))
}

fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn money(c: f64) -> String {
    if c < 0.1 { format!("${c:.3}") } else { format!("${c:.2}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(cost: f64, unknown: bool, requests: u32) -> DayUsage {
        DayUsage { input: 12_000, output: 400, cost, requests, has_unknown_cost: unknown, ..DayUsage::default() }
    }

    #[test]
    fn nothing_is_shown_before_the_first_call() {
        assert_eq!(line(&usage(0.0, false, 0)), None);
    }

    #[test]
    fn a_known_cost_is_shown_and_an_unknown_one_says_so_never_zero() {
        assert_eq!(line(&usage(0.034, false, 1)).as_deref(), Some("this thread: 12.4K tokens, $0.034"));
        assert_eq!(line(&usage(0.0, true, 1)).as_deref(), Some("this thread: 12.4K tokens, cost unknown"));
        assert!(line(&usage(0.5, true, 2)).is_some_and(|l| l.contains("$0.50+")));
    }
}
