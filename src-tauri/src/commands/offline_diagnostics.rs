//! DB-independent flight-recorder inspection at the earliest ordinary CLI boundary.
//! Declared roles: orchestration, accessor, formatter, validator.

use oulipoly_state::diagnostic_recorder::{
    CoalescedReport, DiagnosticId, FlightRecorderReader, InspectionReport, default_recorder_root,
};
use oulipoly_state::event_store::{ReadLimits, TraceId};
use oulipoly_state::longitudinal_metrics::{
    MetricQuery, MetricQueryReport, TraceQueryReport, default_event_store_root, query_metrics,
    query_trace,
};
use serde::Serialize;

#[derive(Serialize)]
struct RecentOutput<'a> {
    command: &'static str,
    limit: usize,
    recent: &'a InspectionReport,
    coalesced: &'a CoalescedReport,
}

#[derive(Serialize)]
struct TraceOutput<'a> {
    command: &'static str,
    diagnostic_id: &'a str,
    trace: &'a InspectionReport,
}

#[derive(Serialize)]
struct MetricsOutput<'a> {
    command: &'static str,
    minutes: u64,
    metrics: &'a MetricQueryReport,
}

#[derive(Serialize)]
struct EventTraceOutput<'a> {
    command: &'static str,
    trace_id: &'a str,
    trace: &'a TraceQueryReport,
}

pub(crate) fn run_recent(limit: usize, json: bool) -> Result<i32, String> {
    let reader = reader()?;
    let views = reader.recent_and_coalesced_failures(limit);
    let output = RecentOutput {
        command: "diagnostics recent",
        limit,
        recent: &views.raw,
        coalesced: &views.coalesced,
    };
    render_output(&output, json, |output| {
        println!("diagnostics recent limit={}", output.limit);
        render_human_section("recent failures", output.recent)?;
        render_human_section("coalesced failures", output.coalesced)
    })?;
    Ok(0)
}

pub(crate) fn run_trace(diagnostic_id: &str, json: bool) -> Result<i32, String> {
    let parsed = diagnostic_id
        .parse::<DiagnosticId>()
        .map_err(|error| format!("invalid diagnostic ID {diagnostic_id:?}: {error}"))?;
    let reader = reader()?;
    let trace = reader.trace(&parsed);
    let output = TraceOutput {
        command: "diagnostics trace",
        diagnostic_id,
        trace: &trace,
    };
    render_output(&output, json, |output| {
        println!("diagnostics trace diagnostic_id={}", output.diagnostic_id);
        render_human_section("trace", output.trace)
    })?;
    Ok(0)
}

pub(crate) fn run_metrics(minutes: u64, json: bool) -> Result<i32, String> {
    let query = MetricQuery::recent(minutes)?;
    let metrics = query_metrics(&default_event_store_root()?, &query)?;
    let output = MetricsOutput {
        command: "diagnostics metrics",
        minutes,
        metrics: &metrics,
    };
    render_output(&output, json, |output| {
        println!("diagnostics metrics minutes={}", output.minutes);
        render_human_section("metrics", output.metrics)
    })?;
    Ok(0)
}

pub(crate) fn run_event_trace(trace_id: &str, json: bool) -> Result<i32, String> {
    let parsed = uuid::Uuid::parse_str(trace_id)
        .map(TraceId::from)
        .map_err(|error| format!("invalid event trace ID {trace_id:?}: {error}"))?;
    let limits = ReadLimits::new(256, 20_000, 64 * 1024 * 1024).map_err(str::to_string)?;
    let trace = query_trace(&default_event_store_root()?, parsed, &limits)?;
    let output = EventTraceOutput {
        command: "diagnostics event-trace",
        trace_id,
        trace: &trace,
    };
    render_output(&output, json, |output| {
        println!("diagnostics event-trace trace_id={}", output.trace_id);
        render_human_section("event trace", output.trace)
    })?;
    Ok(0)
}

fn reader() -> Result<FlightRecorderReader, String> {
    default_recorder_root().map(FlightRecorderReader::new)
}

fn render_output<T, F>(output: &T, json: bool, human: F) -> Result<(), String>
where
    T: Serialize,
    F: FnOnce(&T) -> Result<(), String>,
{
    if json {
        let rendered = serde_json::to_string_pretty(output)
            .map_err(|error| format!("Failed to serialize offline diagnostics JSON: {error}"))?;
        println!("{rendered}");
        return Ok(());
    }
    human(output)
}

fn render_human_section(label: &str, value: &impl Serialize) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to render offline diagnostics {label}: {error}"))?;
    println!("{label}:");
    for line in rendered.lines() {
        println!("  {line}");
    }
    Ok(())
}
