//! Converts nextest/JUnit results into durable test-root OTLP spans.
//!
//! The converter is intentionally separate from a test process: it covers
//! killed or abruptly exited tests whose in-process telemetry cannot flush.

use std::fs;
use std::path::Path;

use anyhow::Context as _;
use opentelemetry::trace::{Span as _, Status, Tracer as _};
use opentelemetry::{KeyValue, global};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use playground_telemetry::semconv;

const FAILURE_MESSAGE: &str = "test.failure.message";
const FAILURE_STACK: &str = "test.failure.stacktrace";
const TEST_ATTEMPT: &str = "test.attempt.ordinal";
const TEST_CODE_REFERENCE: &str = "test.code_reference";
const TEST_CONFIGURATION_OS: &str = "test.configuration.os";
const TEST_CONFIGURATION_ENVIRONMENT: &str = "test.configuration.environment";

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Summary {
    pub(super) total: usize,
    pub(super) passed: usize,
    pub(super) failed: usize,
    pub(super) errors: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct Case {
    suite: String,
    name: String,
    class_name: Option<String>,
    duration_ms: Option<u64>,
    outcome: Outcome,
    diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    Error,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Diagnostic {
    kind: String,
    message: String,
    stack: String,
}

struct ActiveCase {
    case: Case,
    diagnostic_kind: Option<String>,
    diagnostic_message: String,
    diagnostic_stack: String,
    reading_diagnostic: bool,
}

pub(super) fn emit(path: &Path) -> anyhow::Result<Summary> {
    let document = fs::read_to_string(path)
        .with_context(|| format!("failed to read JUnit report {}", path.display()))?;
    let cases = parse(&document).context("failed to parse JUnit report")?;
    let summary = summarize(&cases);
    for case in cases {
        emit_case(case);
    }
    Ok(summary)
}

fn emit_case(case: Case) {
    let code_reference = code_reference(
        &case,
        std::env::var("NEXTEST_BINARY_ID").ok().as_deref(),
        std::env::var("NEXTEST_TEST_NAME").ok().as_deref(),
    );
    let mut attributes = vec![
        KeyValue::new(semconv::TEST_CASE_NAME, case.name.clone()),
        KeyValue::new(semconv::TEST_CASE_RESULT_STATUS, case.outcome.as_str()),
        KeyValue::new(semconv::TEST_SUITE_NAME, case.suite.clone()),
        KeyValue::new(
            semconv::TEST_SUITE_RUN_STATUS,
            if case.outcome == Outcome::Pass {
                "pass"
            } else {
                "fail"
            },
        ),
        KeyValue::new(semconv::CICD_PIPELINE_TASK_TYPE, "test"),
        KeyValue::new(
            semconv::CICD_PIPELINE_RUN_ID,
            std::env::var("CI_RUN_ID").unwrap_or_else(|_| "local".into()),
        ),
        KeyValue::new(
            semconv::PARALLAX_TEST_ID,
            explicit_test_id().unwrap_or_else(|| code_reference.clone()),
        ),
        KeyValue::new(TEST_CODE_REFERENCE, code_reference),
        KeyValue::new(TEST_CONFIGURATION_OS, std::env::consts::OS),
        KeyValue::new(
            TEST_CONFIGURATION_ENVIRONMENT,
            std::env::var("PARALLAX_ENV").unwrap_or_else(|_| "playground".into()),
        ),
        KeyValue::new(
            TEST_ATTEMPT,
            std::env::var("NEXTEST_ATTEMPT")
                .ok()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(1),
        ),
    ];
    if let Some(duration_ms) = case.duration_ms {
        attributes.push(KeyValue::new("test.case.duration_ms", duration_ms as i64));
    }
    if let Some(diagnostic) = &case.diagnostic {
        attributes.push(KeyValue::new(FAILURE_MESSAGE, diagnostic.message.clone()));
        if !diagnostic.stack.is_empty() {
            attributes.push(KeyValue::new(FAILURE_STACK, diagnostic.stack.clone()));
        }
    }

    let tracer = global::tracer("playground.test-report");
    let mut span = tracer.start_with_context("test.case", &playground_telemetry::current_context());
    span.set_attributes(attributes);
    if let Some(diagnostic) = case.diagnostic {
        span.set_status(Status::error(diagnostic.message.clone()));
        span.add_event(
            "test.failure",
            vec![
                KeyValue::new("exception.type", diagnostic.kind),
                KeyValue::new("exception.message", diagnostic.message),
                KeyValue::new("exception.stacktrace", diagnostic.stack),
            ],
        );
    }
    span.end();
}

fn explicit_test_id() -> Option<String> {
    std::env::var(
        semconv::PARALLAX_TEST_ID
            .to_ascii_uppercase()
            .replace('.', "_"),
    )
    .ok()
    .filter(|value| !value.trim().is_empty())
}

fn code_reference(case: &Case, binary_id: Option<&str>, test_name: Option<&str>) -> String {
    match (
        binary_id.filter(|value| !value.is_empty()),
        test_name.filter(|value| !value.is_empty()),
    ) {
        (Some(binary), Some(test)) => format!("{binary}::{test}"),
        _ => case.class_name.as_ref().map_or_else(
            || format!("{}::{}", case.suite, case.name),
            |class| format!("{class}::{}", case.name),
        ),
    }
}

fn parse(document: &str) -> anyhow::Result<Vec<Case>> {
    let mut reader = Reader::from_str(document);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut suites = Vec::new();
    let mut active_case: Option<ActiveCase> = None;
    let mut cases = Vec::new();

    loop {
        buffer.clear();
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) if event.name().as_ref() == b"testsuite" => {
                suites.push(attribute(&event, b"name")?.unwrap_or_else(|| "JUnit".into()));
            }
            Event::End(event) if event.name().as_ref() == b"testsuite" => {
                suites.pop();
            }
            Event::Start(event) if event.name().as_ref() == b"testcase" => {
                active_case = Some(ActiveCase {
                    case: Case {
                        suite: suites.last().cloned().unwrap_or_else(|| "JUnit".into()),
                        name: attribute(&event, b"name")?.unwrap_or_else(|| "unnamed".into()),
                        class_name: attribute(&event, b"classname")?,
                        duration_ms: seconds_to_ms(attribute(&event, b"time")?.as_deref()),
                        outcome: Outcome::Pass,
                        diagnostic: None,
                    },
                    diagnostic_kind: None,
                    diagnostic_message: String::new(),
                    diagnostic_stack: String::new(),
                    reading_diagnostic: false,
                });
            }
            Event::Empty(event) if event.name().as_ref() == b"testcase" => {
                cases.push(Case {
                    suite: suites.last().cloned().unwrap_or_else(|| "JUnit".into()),
                    name: attribute(&event, b"name")?.unwrap_or_else(|| "unnamed".into()),
                    class_name: attribute(&event, b"classname")?,
                    duration_ms: seconds_to_ms(attribute(&event, b"time")?.as_deref()),
                    outcome: Outcome::Pass,
                    diagnostic: None,
                });
            }
            Event::Start(event)
                if event.name().as_ref() == b"failure" || event.name().as_ref() == b"error" =>
            {
                if let Some(active) = active_case.as_mut() {
                    set_diagnostic(active, &event)?;
                    active.reading_diagnostic = true;
                }
            }
            Event::Empty(event)
                if event.name().as_ref() == b"failure" || event.name().as_ref() == b"error" =>
            {
                if let Some(active) = active_case.as_mut() {
                    set_diagnostic(active, &event)?;
                }
            }
            Event::Text(text) => {
                if let Some(active) = active_case.as_mut()
                    && active.reading_diagnostic
                {
                    active.diagnostic_stack.push_str(&text.decode()?);
                }
            }
            Event::End(event)
                if event.name().as_ref() == b"failure" || event.name().as_ref() == b"error" =>
            {
                if let Some(active) = active_case.as_mut()
                    && active.diagnostic_message.is_empty()
                {
                    active.diagnostic_message = active.diagnostic_stack.clone();
                }
                if let Some(active) = active_case.as_mut() {
                    active.reading_diagnostic = false;
                }
            }
            Event::End(event) if event.name().as_ref() == b"testcase" => {
                if let Some(mut active) = active_case.take() {
                    if let Some(kind) = active.diagnostic_kind.take() {
                        active.case.diagnostic = Some(Diagnostic {
                            kind,
                            message: active.diagnostic_message,
                            stack: active.diagnostic_stack,
                        });
                    }
                    cases.push(active.case);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(cases)
}

fn set_diagnostic(active: &mut ActiveCase, event: &BytesStart<'_>) -> anyhow::Result<()> {
    active.case.outcome = if event.name().as_ref() == b"failure" {
        Outcome::Fail
    } else {
        Outcome::Error
    };
    active.diagnostic_kind = Some(
        attribute(event, b"type")?
            .unwrap_or_else(|| String::from_utf8_lossy(event.name().as_ref()).into_owned()),
    );
    active.diagnostic_message = attribute(event, b"message")?.unwrap_or_default();
    Ok(())
}

fn attribute(event: &BytesStart<'_>, wanted: &[u8]) -> anyhow::Result<Option<String>> {
    for attribute in event.attributes().with_checks(false) {
        let attribute = attribute?;
        if attribute.key.as_ref() == wanted {
            return Ok(Some(attribute.unescape_value()?.into_owned()));
        }
    }
    Ok(None)
}

fn seconds_to_ms(value: Option<&str>) -> Option<u64> {
    value
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * 1_000.0).round() as u64)
}

fn summarize(cases: &[Case]) -> Summary {
    let mut summary = Summary {
        total: cases.len(),
        ..Summary::default()
    };
    for case in cases {
        match case.outcome {
            Outcome::Pass => summary.passed += 1,
            Outcome::Fail => summary.failed += 1,
            Outcome::Error => summary.errors += 1,
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::{Case, Outcome, code_reference, parse, seconds_to_ms, summarize};

    #[test]
    fn parses_nested_suites_and_diagnostics() {
        let cases = parse(
            r#"<testsuites><testsuite name="workspace"><testsuite name="crate"><testcase name="passes" classname="crate::tests" time="0.015"/><testcase name="assertion"><failure type="assertion" message="expected true">stack line</failure></testcase><testcase name="harness"><error type="timeout">timed out</error></testcase></testsuite></testsuite></testsuites>"#,
        )
        .expect("valid JUnit parses");
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].suite, "crate");
        assert_eq!(cases[0].duration_ms, Some(15));
        assert_eq!(cases[1].outcome, Outcome::Fail);
        assert_eq!(
            cases[1].diagnostic.as_ref().expect("failure").stack,
            "stack line"
        );
        assert_eq!(cases[2].outcome, Outcome::Error);
        assert_eq!(
            cases[2].diagnostic.as_ref().expect("error").message,
            "timed out"
        );
        assert_eq!(summarize(&cases).errors, 1);
    }

    #[test]
    fn rejects_invalid_duration_without_losing_case() {
        let cases =
            parse(r#"<testsuite name="suite"><testcase name="case" time="nan"/></testsuite>"#)
                .expect("valid JUnit parses");
        assert_eq!(cases[0].duration_ms, None);
        assert_eq!(seconds_to_ms(Some("1.234")), Some(1234));
    }

    #[test]
    fn parses_self_closing_failures_without_absorbing_system_output() {
        let cases = parse(
            r#"<testsuite name="suite"><testcase name="case"><failure type="assertion" message="nope"/><system-out>not a stack</system-out></testcase></testsuite>"#,
        )
        .expect("valid JUnit parses");
        let diagnostic = cases[0].diagnostic.as_ref().expect("failure");
        assert_eq!(cases[0].outcome, Outcome::Fail);
        assert_eq!(diagnostic.message, "nope");
        assert!(diagnostic.stack.is_empty());
    }

    #[test]
    fn uses_nextest_code_reference_without_configuration_identity() {
        let case = Case {
            suite: "suite".into(),
            name: "case".into(),
            class_name: Some("fallback::class".into()),
            duration_ms: None,
            outcome: Outcome::Pass,
            diagnostic: None,
        };
        assert_eq!(
            code_reference(
                &case,
                Some("pricing::bin/pricing"),
                Some("tests::quote[usd]")
            ),
            "pricing::bin/pricing::tests::quote[usd]"
        );
        assert_eq!(code_reference(&case, None, None), "fallback::class::case");
    }
}
