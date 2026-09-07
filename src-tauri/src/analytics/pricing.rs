//! USD per million tokens.
//!
//! **Provenance: transcribed on 2026-09-08 from Anthropic's published list
//! prices at <https://platform.claude.com/docs/en/about-claude/pricing>**
//! (the "Model pricing" and "Prompt caching" tables on that page). These are
//! first-party Claude API rates; Bedrock and Google Cloud are partner-priced
//! and are not represented here.
//!
//! The same page states the cache multipliers this table's derived columns
//! follow: a 5-minute cache write is 1.25× base input, a 1-hour cache write
//! 2×, and a cache read 0.1× — *except* on Claude Fable 5.1 and Claude Mythos
//! 5.1, where a cache read is 0.025× of base input, i.e. a flat $0.25/MTok.
//! Claude Fable 5 and Claude Mythos 5 are **not** in that exception: their
//! cache reads are the ordinary 0.1×, i.e. $1.00/MTok. Grouping them with the
//! `.1` generation undercounts their cache reads fourfold, which is why they
//! have their own row rather than sharing 5.1's.
//!
//! **These prices will drift, and a wrong price here is invisible** — a
//! dollar figure that is wrong looks exactly like one that is right. Two
//! tests exist to make drift loud rather than silent:
//! `the_published_list_prices_are_what_ship` pins every figure below to a
//! literal, and `derived_rates_follow_the_published_multipliers` re-derives
//! the three cache columns from base input. Both iterate `TABLE` itself, so a
//! model added here is covered the moment it is added. Changing a price means
//! re-reading the page above, updating the canary, and moving the date in
//! this comment — not editing one number in one place.
//!
//! Everything derived from this table is labelled an *estimate* in the UI
//! (spec §11.2): it applies API list prices to subscription usage, which is
//! not what the user is billed.

/// USD per million tokens, for one model, across the five ways a token can be
/// charged. Every field is a list price or a published multiple of one — see
/// this module's provenance note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

/// Every model this build can price, and nothing else.
///
/// A slice rather than a `match` so the drift tests below can iterate it. A
/// `match` would force the tests to keep their own hand-written list of model
/// ids beside it, and a model added to the match but forgotten in that list
/// would then ship unchecked. Here there is one list, and it is the one
/// `for_model` reads.
///
/// Every row spells out all five figures even where two models share them, so
/// a reviewer can read this table straight down against Anthropic's published
/// one instead of resolving shared constants — and so a model whose price
/// later diverges from its neighbour's is a one-row edit, not a regrouping.
const TABLE: &[(&str, Price)] = &[
    // The `.1` generation, and only it, reads cache at 0.025x base input.
    (
        "claude-fable-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write_5m: 12.50,
            cache_write_1h: 20.0,
            cache_read: 0.25,
        },
    ),
    (
        "claude-mythos-5-1",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write_5m: 12.50,
            cache_write_1h: 20.0,
            cache_read: 0.25,
        },
    ),
    // Fable 5 and Mythos 5: same input and output as 5.1, but cache reads at
    // the ordinary 0.1x - $1.00, four times the 5.1 rate.
    (
        "claude-fable-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write_5m: 12.50,
            cache_write_1h: 20.0,
            cache_read: 1.0,
        },
    ),
    (
        "claude-mythos-5",
        Price {
            input: 10.0,
            output: 50.0,
            cache_write_5m: 12.50,
            cache_write_1h: 20.0,
            cache_read: 1.0,
        },
    ),
    (
        "claude-opus-5",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write_5m: 6.25,
            cache_write_1h: 10.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-opus-4-8",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write_5m: 6.25,
            cache_write_1h: 10.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-opus-4-7",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write_5m: 6.25,
            cache_write_1h: 10.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-opus-4-6",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write_5m: 6.25,
            cache_write_1h: 10.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-opus-4-5",
        Price {
            input: 5.0,
            output: 25.0,
            cache_write_5m: 6.25,
            cache_write_1h: 10.0,
            cache_read: 0.50,
        },
    ),
    (
        "claude-sonnet-5",
        Price {
            input: 2.0,
            output: 10.0,
            cache_write_5m: 2.50,
            cache_write_1h: 4.0,
            cache_read: 0.20,
        },
    ),
    (
        "claude-sonnet-4-6",
        Price {
            input: 3.0,
            output: 15.0,
            cache_write_5m: 3.75,
            cache_write_1h: 6.0,
            cache_read: 0.30,
        },
    ),
    (
        "claude-sonnet-4-5",
        Price {
            input: 3.0,
            output: 15.0,
            cache_write_5m: 3.75,
            cache_write_1h: 6.0,
            cache_read: 0.30,
        },
    ),
    (
        "claude-haiku-4-5",
        Price {
            input: 1.0,
            output: 5.0,
            cache_write_5m: 1.25,
            cache_write_1h: 2.0,
            cache_read: 0.10,
        },
    ),
];

/// The price for `model`, or `None` when this build has never heard of it.
///
/// `None`, never `Some(zero)`. A zero-priced model would add cleanly into
/// every total and vanish; the `None` is what lets `analytics::summarize`
/// count the model's tokens while keeping them out of the dollar figure, and
/// say so.
pub fn for_model(model: &str) -> Option<Price> {
    let model = strip_snapshot_date(model);
    TABLE
        .iter()
        .find(|(id, _)| *id == model)
        .map(|(_, price)| *price)
}

/// `claude-haiku-4-5-20251001` → `claude-haiku-4-5`.
///
/// Transcripts record whichever id the request resolved to, and for models
/// before the 4.6 generation that is the dated snapshot rather than the
/// alias. It is not hypothetical: `claude-haiku-4-5-20251001` was the one
/// dated id present across this machine's `~/.claude/projects` when this was
/// written, on 247 of 31,000 assistant lines carrying usage, every other
/// model appearing in its dateless form. Anthropic's model-ids page
/// documents the alias as a pointer to exactly that dated id, so the two name
/// one model at one price, and matching only the alias would drop those lines
/// into the unpriced pile for no reason.
///
/// Deliberately narrow: an eight-digit trailing segment and nothing else. A
/// looser rule would eat the `-1` of `claude-fable-5-1` and price it as
/// `claude-fable-5`, which is the fourfold cache-read error this module
/// exists to avoid.
fn strip_snapshot_date(model: &str) -> &str {
    match model.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published list prices, as read from
    /// <https://platform.claude.com/docs/en/about-claude/pricing> on
    /// 2026-09-08: `(model, input, output, 5m write, 1h write, cache read)`.
    ///
    /// A second copy of `TABLE` on purpose. It is the canary this module's
    /// provenance note points at: an edit to a price in `TABLE` that is not
    /// also made here fails loudly and names the model, so re-pricing the
    /// user's whole history takes a deliberate two-place change and a fresh
    /// look at the page, rather than one silent keystroke.
    const PUBLISHED: &[(&str, f64, f64, f64, f64, f64)] = &[
        ("claude-fable-5-1", 10.0, 50.0, 12.50, 20.0, 0.25),
        ("claude-mythos-5-1", 10.0, 50.0, 12.50, 20.0, 0.25),
        ("claude-fable-5", 10.0, 50.0, 12.50, 20.0, 1.0),
        ("claude-mythos-5", 10.0, 50.0, 12.50, 20.0, 1.0),
        ("claude-opus-5", 5.0, 25.0, 6.25, 10.0, 0.50),
        ("claude-opus-4-8", 5.0, 25.0, 6.25, 10.0, 0.50),
        ("claude-opus-4-7", 5.0, 25.0, 6.25, 10.0, 0.50),
        ("claude-opus-4-6", 5.0, 25.0, 6.25, 10.0, 0.50),
        ("claude-opus-4-5", 5.0, 25.0, 6.25, 10.0, 0.50),
        ("claude-sonnet-5", 2.0, 10.0, 2.50, 4.0, 0.20),
        ("claude-sonnet-4-6", 3.0, 15.0, 3.75, 6.0, 0.30),
        ("claude-sonnet-4-5", 3.0, 15.0, 3.75, 6.0, 0.30),
        ("claude-haiku-4-5", 1.0, 5.0, 1.25, 2.0, 0.10),
    ];

    /// The two models whose cache reads are 0.025× base input rather than the
    /// 0.1× every other model uses. Exactly two, and not their `-5`
    /// predecessors — see this module's provenance note.
    const QUARTER_RATE_CACHE_READS: &[&str] = &["claude-fable-5-1", "claude-mythos-5-1"];

    #[test]
    fn the_published_list_prices_are_what_ship() {
        for (model, input, output, write_5m, write_1h, read) in PUBLISHED {
            let price = for_model(model).unwrap_or_else(|| panic!("{model} is not priced"));
            assert_eq!(
                price,
                Price {
                    input: *input,
                    output: *output,
                    cache_write_5m: *write_5m,
                    cache_write_1h: *write_1h,
                    cache_read: *read,
                },
                "{model}"
            );
        }
    }

    /// The canary above pins the numbers; this pins the *relationships* the
    /// pricing page states, for every row of `TABLE` rather than for the rows
    /// someone remembered to list. It is the test that catches the mistake
    /// this table has already made once: pricing Fable 5 and Mythos 5 like
    /// 5.1 gives them a $0.25 cache read where 0.1 × $10.00 is $1.00, and
    /// this fails naming the model.
    ///
    /// It also catches the subtler direction — an input price updated on its
    /// own, leaving the three cache columns quoting the old rate.
    ///
    /// Compared within 1e-9 rather than exactly, because `3.0 * 0.1` is
    /// `0.30000000000000004` in binary floating point while the published
    /// Sonnet 4.x cache read is `$0.30`. The tolerance is eight orders of
    /// magnitude below the smallest published increment ($0.005/MTok), so it
    /// forgives the representation and nothing else: the error this test
    /// exists for — a $0.25 cache read where $1.00 is published — is off by
    /// 0.75.
    #[test]
    fn derived_rates_follow_the_published_multipliers() {
        fn assert_rate(model: &str, column: &str, actual: f64, expected: f64) {
            assert!(
                (actual - expected).abs() < 1e-9,
                "{model}: {column} is {actual}, but the published multiplier gives {expected}"
            );
        }

        for (model, price) in TABLE {
            assert_rate(
                model,
                "the 5-minute cache write (1.25x base input)",
                price.cache_write_5m,
                price.input * 1.25,
            );
            assert_rate(
                model,
                "the 1-hour cache write (2x base input)",
                price.cache_write_1h,
                price.input * 2.0,
            );
            let multiplier = if QUARTER_RATE_CACHE_READS.contains(model) {
                0.025
            } else {
                0.1
            };
            assert_rate(
                model,
                &format!("the cache read ({multiplier}x base input)"),
                price.cache_read,
                price.input * multiplier,
            );
        }
    }

    /// Stated as a ratio rather than two absolute figures, so it cannot be
    /// satisfied by both rows being edited wrong in the same direction — the
    /// exact way these two families got conflated in the first place.
    #[test]
    fn fable_and_mythos_5_read_cache_at_four_times_the_5_1_rate() {
        for (five, five_one) in [
            ("claude-fable-5", "claude-fable-5-1"),
            ("claude-mythos-5", "claude-mythos-5-1"),
        ] {
            let older = for_model(five).unwrap().cache_read;
            let newer = for_model(five_one).unwrap().cache_read;
            assert_eq!(older, newer * 4.0, "{five} vs {five_one}");
        }
    }

    /// The whole point of the `Option`. A model this build has never heard of
    /// must not resolve to a price at all — a `Some(Price { .. 0.0 })` would
    /// add into every bucket and every total as a clean zero and be
    /// indistinguishable from a model that genuinely cost nothing.
    ///
    /// `<synthetic>` is not invented: Claude Code writes it as the model on
    /// its own synthesised assistant turns, and it appears in real
    /// transcripts.
    #[test]
    fn a_model_this_build_cannot_price_is_none_rather_than_free() {
        for model in [
            "some-future-model",
            "",
            "<synthetic>",
            "claude-opus-6",
            "gpt-5",
        ] {
            assert!(for_model(model).is_none(), "{model} must not be priced");
        }
    }

    /// A plausible next id must not inherit the price of the one it extends.
    /// `claude-opus-5-1` sharing `claude-opus-5`'s row is exactly how a model
    /// gets mispriced by a prefix or fuzzy match; unpriced-and-surfaced is the
    /// safe answer, and adding the real row later is a one-line change.
    #[test]
    fn a_near_miss_id_does_not_borrow_a_neighbours_price() {
        for model in [
            "claude-opus-5-1",
            "claude-sonnet-5-1",
            "claude-haiku-5",
            "claude-fable-5-2",
        ] {
            assert!(for_model(model).is_none(), "{model} must not be priced");
        }
    }

    /// The dated snapshot ids that really appear in transcripts resolve to
    /// their alias's price, and nothing looser does.
    #[test]
    fn a_dated_snapshot_id_prices_as_its_alias() {
        for (dated, alias) in [
            ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
            ("claude-opus-4-5-20251101", "claude-opus-4-5"),
            ("claude-sonnet-4-5-20250929", "claude-sonnet-4-5"),
        ] {
            let dated_price = for_model(dated);
            assert!(dated_price.is_some(), "{dated} must be priced");
            assert_eq!(dated_price, for_model(alias), "{dated} vs {alias}");
        }
    }

    /// Only an eight-digit trailing segment is a date. Seven or nine digits
    /// are not, and neither is the `-1` that distinguishes a `.1` generation
    /// — the case where a looser rule would silently reprice Fable 5.1's
    /// cache reads at four times their real rate.
    #[test]
    fn only_an_eight_digit_tail_is_treated_as_a_snapshot_date() {
        assert!(for_model("claude-haiku-4-5-2025100").is_none());
        assert!(for_model("claude-haiku-4-5-202510011").is_none());
        assert!(for_model("claude-haiku-4-5-2025").is_none());
        assert_eq!(strip_snapshot_date("claude-fable-5-1"), "claude-fable-5-1");
        assert_eq!(
            for_model("claude-fable-5-1").unwrap().cache_read,
            0.25,
            "the 5.1 rate must survive the snapshot-date rule"
        );
    }
}
