//! Parameterized-name test fixture.
//!
//! Rust test function paths cannot contain `[…]`, so this custom harness
//! registers bracketed case names directly. `playground test-report` derives
//! `test.case.parameters` from the bracket suffix, giving the rust stack the
//! same parameterized-test evidence the java (JUnit 5 `@ParameterizedTest`)
//! and web (Playwright projects) stacks emit.

use libtest_mimic::{Arguments, Trial};

fn quote(currency: &str, tier: &str) -> u32 {
    let base = match currency {
        "usd" => 100,
        "eur" => 92,
        other => panic!("unsupported currency {other}"),
    };
    match tier {
        "basic" => base,
        "pro" => base * 2,
        other => panic!("unsupported tier {other}"),
    }
}

fn main() {
    let args = Arguments::from_args();
    let cases = [
        ("usd", "basic", 100),
        ("usd", "pro", 200),
        ("eur", "pro", 184),
    ];
    let trials = cases
        .into_iter()
        .map(|(currency, tier, expected)| {
            Trial::test(format!("quote[{currency}, {tier}]"), move || {
                let actual = quote(currency, tier);
                if actual == expected {
                    Ok(())
                } else {
                    Err(format!("quote({currency}, {tier}) = {actual}, expected {expected}").into())
                }
            })
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}
