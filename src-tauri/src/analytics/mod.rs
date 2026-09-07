//! Local token and cost analytics: an estimate built from Claude Code's own
//! transcripts, off by default, and the only part of this app that reads a
//! data source other than the usage endpoint.
//!
//! The number this module produces is one the user may believe, and a dollar
//! figure that is wrong looks exactly like one that is right. Two things
//! follow from that, and they are the reason for most of the shape below:
//!
//! - Nothing is priced at a guess. `pricing::for_model` answers `None` for a
//!   model this build has never heard of, and that `None` travels all the way
//!   to `Bucket::unpriced_tokens` and `Summary::unpriced_tokens` so the UI can
//!   say the estimate is missing something. Silently costing an unknown model
//!   at zero would be indistinguishable from a model that was genuinely free.
//! - It is an *estimate*, said in the UI and not only here: it applies API
//!   list prices to subscription usage, which is not what the user is billed
//!   (spec §11.2).

pub mod pricing;
pub mod scan;

use std::collections::HashMap;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Serialize;

/// One row of a breakdown — a model, a project, or a day.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub name: String,
    pub tokens: u64,
    /// The priced part only. When `unpriced_tokens` is non-zero this is a
    /// lower bound, not a total, and the UI must not present it as one.
    pub cost: f64,
    /// How many of `tokens` had no price in this build. Zero for a bucket
    /// whose models are all priced; equal to `tokens` for a model bucket that
    /// is entirely unpriced; somewhere between for a project or a day that
    /// mixes the two.
    pub unpriced_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub by_model: Vec<Bucket>,
    pub by_project: Vec<Bucket>,
    /// Newest day first, unlike the cost-ranked buckets above.
    pub by_day: Vec<Bucket>,
    /// The priced part only — see `unpriced_tokens`.
    pub total_cost: f64,
    pub total_tokens: u64,
    /// Tokens spent on models this build cannot price, and which `total_cost`
    /// therefore excludes. Non-zero means the estimate is a floor; the tab
    /// says so and names the models responsible.
    pub unpriced_tokens: u64,
}

/// A running (tokens, cost, unpriced tokens) triple for one bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Totals {
    tokens: u64,
    cost: f64,
    unpriced_tokens: u64,
}

impl Totals {
    /// `cost` is `None` for an entry whose model has no price. The tokens are
    /// counted either way — they were really spent — but an unpriced entry
    /// adds nothing to `cost` and its tokens are recorded separately, so that
    /// no caller can mistake "cost nothing" for "not counted".
    fn add(&mut self, tokens: u64, cost: Option<f64>) {
        self.tokens += tokens;
        match cost {
            Some(cost) => self.cost += cost,
            None => self.unpriced_tokens += tokens,
        }
    }
}

/// What one entry cost, or `None` when its model is not in the price table.
///
/// `None` rather than `0.0` is the whole point: a zero would add cleanly into
/// every bucket and every total and then be impossible to tell apart from a
/// genuinely free request.
fn cost_of(entry: &scan::Entry) -> Option<f64> {
    let price = pricing::for_model(&entry.model)?;
    let per_million = |tokens: u64, rate: f64| tokens as f64 / 1_000_000.0 * rate;
    Some(
        per_million(entry.input, price.input)
            + per_million(entry.output, price.output)
            + per_million(entry.cache_write_5m, price.cache_write_5m)
            + per_million(entry.cache_write_1h, price.cache_write_1h)
            + per_million(entry.cache_read, price.cache_read),
    )
}

fn tokens_of(entry: &scan::Entry) -> u64 {
    entry.input + entry.output + entry.cache_write_5m + entry.cache_write_1h + entry.cache_read
}

/// Which calendar day an instant belongs to, in `zone`.
///
/// Timestamps are stored UTC, but "today" is a local idea, and bucketing by
/// the UTC date puts an evening's work on tomorrow's row for everyone east of
/// Greenwich — which is most of Europe for several hours of every day, and the
/// user this ships to. `summarize` passes `Local`; the parameter exists so the
/// behaviour can be pinned against fixed offsets in tests instead of against
/// whatever zone the machine running them happens to be in.
fn day_key<Tz: TimeZone>(timestamp: &DateTime<Utc>, zone: &Tz) -> String {
    timestamp.with_timezone(zone).date_naive().to_string()
}

/// Cost-ranked, most expensive first.
///
/// The final tie-break on name is not cosmetic: `groups` is a `HashMap`, so
/// two buckets with equal cost and equal tokens would otherwise swap places
/// between calls and the list would reshuffle under the user every poll.
fn bucket(groups: HashMap<String, Totals>) -> Vec<Bucket> {
    let mut buckets: Vec<Bucket> = groups
        .into_iter()
        .filter(spent_something)
        .map(into_bucket)
        .collect();
    buckets.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.tokens.cmp(&a.tokens))
            .then_with(|| a.name.cmp(&b.name))
    });
    buckets
}

/// A bucket that spent no tokens says nothing and is not rendered.
///
/// Real transcripts produce these: `<synthetic>` is what Claude Code records
/// as the model on its own synthesised assistant turns, and those report zero
/// of every token class. Kept, such a row would appear in the model list as
/// "0 tokens — not priced", drawing the eye to a warning about nothing, since
/// no price is missing from an estimate when nothing was spent.
fn spent_something((_, totals): &(String, Totals)) -> bool {
    totals.tokens > 0
}

fn into_bucket((name, totals): (String, Totals)) -> Bucket {
    Bucket {
        name,
        tokens: totals.tokens,
        cost: totals.cost,
        unpriced_tokens: totals.unpriced_tokens,
    }
}

pub fn summarize(entries: &[scan::Entry]) -> Summary {
    let mut by_model: HashMap<String, Totals> = HashMap::new();
    let mut by_project: HashMap<String, Totals> = HashMap::new();
    let mut by_day: HashMap<String, Totals> = HashMap::new();
    let mut overall = Totals::default();

    for entry in entries {
        let cost = cost_of(entry);
        let tokens = tokens_of(entry);
        overall.add(tokens, cost);

        by_model
            .entry(entry.model.clone())
            .or_default()
            .add(tokens, cost);
        by_project
            .entry(entry.project.clone())
            .or_default()
            .add(tokens, cost);
        by_day
            .entry(day_key(&entry.timestamp, &Local))
            .or_default()
            .add(tokens, cost);
    }

    // Days read as a timeline, so they sort by date rather than by cost. The
    // key is a `NaiveDate`'s ISO form — fixed-width and zero-padded — so a
    // reverse string sort is a reverse date sort.
    let mut days: Vec<Bucket> = by_day
        .into_iter()
        .filter(spent_something)
        .map(into_bucket)
        .collect();
    days.sort_by(|a, b| b.name.cmp(&a.name));

    Summary {
        by_model: bucket(by_model),
        by_project: bucket(by_project),
        by_day: days,
        total_cost: overall.cost,
        total_tokens: overall.tokens,
        unpriced_tokens: overall.unpriced_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn entry(model: &str, project: &str, input: u64, output: u64) -> scan::Entry {
        scan::Entry {
            request_id: format!("{model}-{project}-{input}"),
            model: model.into(),
            timestamp: Utc::now(),
            project: project.into(),
            input,
            output,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 0,
        }
    }

    /// Each class gets a *different* token count on purpose. With a million of
    /// everything the five rates are interchangeable — swapping the 5m and 1h
    /// cache-write rates, or input and output, would leave the total unmoved and
    /// the test would pass against the bug it exists to catch.
    #[test]
    fn costs_each_token_class_at_its_own_rate() {
        let e = scan::Entry {
            cache_write_5m: 3_000_000,
            cache_write_1h: 4_000_000,
            cache_read: 5_000_000,
            ..entry("claude-opus-5", "alpha", 1_000_000, 2_000_000)
        };
        // 5.00×1 + 25.00×2 + 6.25×3 + 10.00×4 + 0.50×5
        //   = 5.00 + 50.00 + 18.75 + 40.00 + 2.50 = 116.25
        let summary = summarize(&[e]);
        assert!(
            (summary.total_cost - 116.25).abs() < 1e-9,
            "{}",
            summary.total_cost
        );
        assert_eq!(summary.total_tokens, 15_000_000);
    }

    #[test]
    fn unknown_models_contribute_tokens_but_no_cost() {
        let summary = summarize(&[entry("some-future-model", "alpha", 1_000_000, 1_000_000)]);
        assert_eq!(summary.total_cost, 0.0);
        assert_eq!(summary.total_tokens, 2_000_000);
    }

    #[test]
    fn buckets_by_model_and_by_project() {
        let summary = summarize(&[
            entry("claude-opus-5", "alpha", 1_000_000, 0),
            entry("claude-haiku-4-5", "alpha", 1_000_000, 0),
            entry("claude-opus-5", "beta", 1_000_000, 0),
        ]);
        assert_eq!(summary.by_model.len(), 2);
        assert_eq!(summary.by_project.len(), 2);
        // Sorted by cost, descending: opus 2M tokens at $5/M beats haiku 1M at $1/M.
        assert_eq!(summary.by_model[0].name, "claude-opus-5");
        assert!((summary.by_model[0].cost - 10.0).abs() < 1e-9);
        assert_eq!(summary.by_project[0].name, "alpha");
    }

    /// Local noon on a given date, as the UTC instant an `Entry` carries.
    ///
    /// Built from local wall time rather than UTC because `summarize` buckets
    /// by the local date: `Utc.with_ymd_and_hms(.., 23, 0, 0)` is already the
    /// next day in central Europe, so a UTC-built fixture would make these
    /// tests pass or fail according to the timezone of the machine running
    /// them. Noon is the safe hour — DST transitions happen at night, so no
    /// zone makes it ambiguous or non-existent.
    fn local_noon(day: u32) -> chrono::DateTime<Utc> {
        Local
            .with_ymd_and_hms(2026, 9, day, 12, 0, 0)
            .single()
            .expect("local noon is unambiguous in every timezone")
            .with_timezone(&Utc)
    }

    #[test]
    fn days_are_bucketed_newest_first() {
        let mut older = entry("claude-opus-5", "alpha", 1_000_000, 0);
        older.request_id = "older".into();
        older.timestamp = local_noon(5);
        let mut newer = entry("claude-opus-5", "alpha", 1_000_000, 0);
        newer.request_id = "newer".into();
        newer.timestamp = local_noon(7);

        let summary = summarize(&[older, newer]);
        assert_eq!(summary.by_day.len(), 2);
        assert_eq!(summary.by_day[0].name, "2026-09-07");
        assert_eq!(summary.by_day[1].name, "2026-09-05");
    }

    #[test]
    fn entries_on_the_same_day_are_merged() {
        // Noon and eleven hours later — the same local day everywhere, which a
        // pair built from 01:00 and 23:00 UTC is not.
        let mut a = entry("claude-opus-5", "alpha", 1_000_000, 0);
        a.timestamp = local_noon(7) - chrono::Duration::hours(6);
        let mut b = entry("claude-opus-5", "beta", 1_000_000, 0);
        b.timestamp = local_noon(7) + chrono::Duration::hours(5);

        let summary = summarize(&[a, b]);
        assert_eq!(summary.by_day.len(), 1);
        assert_eq!(summary.by_day[0].name, "2026-09-07");
        assert_eq!(summary.by_day[0].tokens, 2_000_000);
    }

    /// A day is the user's day, not UTC's.
    ///
    /// Driven against fixed offsets rather than `Local`, so it asserts the
    /// same thing on a CI runner in UTC as on the machine this ships to. Both
    /// cases are ones the old `%Y-%m-%d` on the UTC timestamp got wrong: at
    /// +02:00, which is central Europe in September, an evening's work landed
    /// on tomorrow's row for the last two hours of every day; at -05:00 the
    /// small hours landed on yesterday's.
    #[test]
    fn a_day_is_the_local_calendar_day_not_the_utc_one() {
        use chrono::FixedOffset;

        let late_evening = Utc.with_ymd_and_hms(2026, 9, 7, 23, 0, 0).unwrap();
        let central_europe = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(day_key(&late_evening, &Utc), "2026-09-07");
        assert_eq!(day_key(&late_evening, &central_europe), "2026-09-08");

        let small_hours = Utc.with_ymd_and_hms(2026, 9, 7, 1, 0, 0).unwrap();
        let new_york = FixedOffset::west_opt(5 * 3600).unwrap();
        assert_eq!(day_key(&small_hours, &Utc), "2026-09-07");
        assert_eq!(day_key(&small_hours, &new_york), "2026-09-06");
    }

    #[test]
    fn an_empty_input_summarizes_to_zero() {
        let summary = summarize(&[]);
        assert_eq!(summary.total_cost, 0.0);
        assert_eq!(summary.total_tokens, 0);
        assert!(summary.by_model.is_empty());
        assert!(summary.by_day.is_empty());
    }

    /// The tokens of an unpriced model are counted and the cost is not — but
    /// the point of this test is the third number. Without
    /// `unpriced_tokens`, a summary in which half the spend ran on a model
    /// this build has never heard of is byte-for-byte identical to one in
    /// which everything was priced and the rest of the machine was idle. The
    /// UI has to be able to tell those apart to say so.
    #[test]
    fn an_unpriced_model_is_reported_rather_than_folded_into_the_total() {
        let summary = summarize(&[
            entry("claude-opus-5", "alpha", 1_000_000, 0),
            entry("some-future-model", "alpha", 4_000_000, 0),
        ]);

        assert!(
            (summary.total_cost - 5.0).abs() < 1e-9,
            "only the priced million: {}",
            summary.total_cost
        );
        assert_eq!(summary.total_tokens, 5_000_000);
        assert_eq!(summary.unpriced_tokens, 4_000_000);

        let priced = summary
            .by_model
            .iter()
            .find(|b| b.name == "claude-opus-5")
            .unwrap();
        assert_eq!(priced.unpriced_tokens, 0);

        let unpriced = summary
            .by_model
            .iter()
            .find(|b| b.name == "some-future-model")
            .unwrap();
        assert_eq!(unpriced.cost, 0.0);
        assert_eq!(
            unpriced.unpriced_tokens, unpriced.tokens,
            "a wholly unpriced model's every token is unpriced"
        );
    }

    /// A project or a day can mix the two, and that is the case a per-model
    /// flag could not express. Such a bucket's `cost` is a floor rather than a
    /// total, and `unpriced_tokens` — strictly between zero and `tokens` — is
    /// what tells the UI to render it as one.
    #[test]
    fn a_bucket_mixing_priced_and_unpriced_models_reports_both() {
        use chrono::TimeZone;
        let day = chrono::Utc.with_ymd_and_hms(2026, 9, 7, 12, 0, 0).unwrap();
        let mut priced = entry("claude-opus-5", "alpha", 1_000_000, 0);
        priced.timestamp = day;
        let mut unpriced = entry("some-future-model", "alpha", 4_000_000, 0);
        unpriced.timestamp = day;

        let summary = summarize(&[priced, unpriced]);

        for bucket in [&summary.by_project[0], &summary.by_day[0]] {
            assert_eq!(bucket.tokens, 5_000_000, "{}", bucket.name);
            assert_eq!(bucket.unpriced_tokens, 4_000_000, "{}", bucket.name);
            assert!(
                bucket.unpriced_tokens < bucket.tokens,
                "{}: a mixed bucket is neither wholly priced nor wholly unpriced",
                bucket.name
            );
            assert!((bucket.cost - 5.0).abs() < 1e-9, "{}", bucket.name);
        }
    }

    /// Every model in the price table costs something, so a summary drawn only
    /// from priced models must report nothing unpriced anywhere.
    ///
    /// What this catches that the two tests above do not is the classification
    /// going wrong in the safe-looking direction: a *priced* model treated as
    /// unpriced. Those tests only ever look at buckets they expect to be
    /// unpriced, so a `cost_of` that returned `None` too eagerly would leave
    /// them green while quietly emptying the estimate and papering it over
    /// with a warning.
    #[test]
    fn a_wholly_priced_summary_reports_nothing_unpriced() {
        let summary = summarize(&[
            entry("claude-opus-5", "alpha", 1_000_000, 0),
            entry("claude-haiku-4-5", "beta", 1_000_000, 0),
        ]);
        assert_eq!(summary.unpriced_tokens, 0);
        for bucket in summary
            .by_model
            .iter()
            .chain(&summary.by_project)
            .chain(&summary.by_day)
        {
            assert_eq!(bucket.unpriced_tokens, 0, "{}", bucket.name);
        }
    }

    /// The webview reads these names, so the serialized shape is part of the
    /// contract with `AnalyticsTab.svelte` — `by_model` must arrive as
    /// `byModel`, and the field the tab needs to warn about unpriced models
    /// must actually be on the wire.
    #[test]
    fn the_summary_serializes_in_the_camel_case_the_tab_reads() {
        let json = serde_json::to_value(summarize(&[entry(
            "some-future-model",
            "alpha",
            1_000_000,
            0,
        )]))
        .unwrap();
        for key in [
            "byModel",
            "byProject",
            "byDay",
            "totalCost",
            "totalTokens",
            "unpricedTokens",
        ] {
            assert!(json.get(key).is_some(), "Summary is missing {key}");
        }
        for key in ["name", "tokens", "cost", "unpricedTokens"] {
            assert!(
                json["byModel"][0].get(key).is_some(),
                "Bucket is missing {key}"
            );
        }
    }

    /// Claude Code records `<synthetic>` as the model on its own synthesised
    /// assistant turns, and those report zero of every token class — so this
    /// is a shape real transcripts produce, not a contrived one. Such a bucket
    /// would render as "0 tokens — not priced", which points a warning at
    /// something that took nothing out of the estimate.
    ///
    /// The entry's tokens still reach the totals, because there are none of
    /// them; what is dropped is the empty row, not the arithmetic.
    #[test]
    fn a_bucket_that_spent_nothing_is_not_listed() {
        let summary = summarize(&[
            entry("claude-opus-5", "alpha", 1_000_000, 0),
            entry("<synthetic>", "alpha", 0, 0),
        ]);

        assert_eq!(
            summary.by_model.len(),
            1,
            "{:?}",
            summary.by_model.iter().map(|b| &b.name).collect::<Vec<_>>()
        );
        assert_eq!(summary.by_model[0].name, "claude-opus-5");
        assert_eq!(summary.total_tokens, 1_000_000);
        assert_eq!(
            summary.unpriced_tokens, 0,
            "a model that spent nothing withholds nothing from the estimate"
        );
    }

    /// The other side of it: a project whose only entry is unpriced but which
    /// did spend tokens must still be listed, or the tab would drop the very
    /// rows the unpriced warning is about.
    #[test]
    fn a_bucket_that_spent_unpriced_tokens_is_still_listed() {
        let summary = summarize(&[entry("some-future-model", "alpha", 2_000_000, 0)]);
        assert_eq!(summary.by_model.len(), 1);
        assert_eq!(summary.by_project.len(), 1);
        assert_eq!(summary.by_day.len(), 1);
        assert_eq!(summary.unpriced_tokens, 2_000_000);
    }
}
