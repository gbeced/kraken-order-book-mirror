use std::error::Error;
use std::fs::File;
use std::hint::black_box;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::level3::mirror;

#[derive(Debug)]
struct Report {
    snapshot_nanos: Option<u64>,
    update_nanos: Vec<u64>,
    book_present: bool,
}

#[derive(Debug)]
struct MeasureError {
    index: usize,
    message: String,
    partial: Report,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: debug build timings are not comparable; use cargo run --release -- bench"
        );
    }

    let (path, depth) = parse_bench_args(std::env::args().skip(1))?;
    let messages = load_messages(&path)?;
    if messages.is_empty() {
        return Err(format!("no messages in {}", path.display()).into());
    }

    match measure(&messages, depth) {
        Ok(report) => {
            print_report(&path, depth, &report);
            Ok(())
        }
        Err(err) => {
            eprintln!("error at message {}: {}", err.index, err.message);
            print_report(&path, depth, &err.partial);
            std::process::exit(1);
        }
    }
}

fn parse_bench_args(args: impl IntoIterator<Item = String>) -> Result<(PathBuf, usize), String> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("bench") {
        return Err("expected bench".to_string());
    }

    let mut path = PathBuf::from("tmp/raw_messages.log");
    let mut depth = 100usize;
    let mut seen_file = false;

    while let Some(arg) = args.next() {
        if arg == "--depth" {
            let value = args
                .next()
                .ok_or_else(|| "--depth needs a number".to_string())?;
            depth = parse_depth(&value)?;
        } else if let Some(value) = arg.strip_prefix("--depth=") {
            depth = parse_depth(value)?;
        } else if arg.starts_with('-') {
            return Err(format!("unknown argument: {arg}"));
        } else if !seen_file {
            path = PathBuf::from(arg);
            seen_file = true;
        } else {
            return Err(format!("unexpected argument: {arg}"));
        }
    }

    Ok((path, depth))
}

fn parse_depth(value: &str) -> Result<usize, String> {
    value.parse().map_err(|_| format!("invalid depth: {value}"))
}

pub fn load_messages(path: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let file =
        File::open(path).map_err(|err| format!("failed to open {}: {err}", path.display()))?;
    let reader = BufReader::with_capacity(1024 * 1024, file);
    let mut messages = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        messages.push(line);
    }
    Ok(messages)
}

fn measure(messages: &[String], depth: usize) -> Result<Report, MeasureError> {
    // Captures are BTC/USD: price tick 0.1, quantity step 1e-8.
    let mut mirror = mirror::OrderBookMirror::new(depth, 1, 8);
    let mut snapshot_nanos = None;
    let mut update_nanos = Vec::with_capacity(messages.len());

    for (index, msg) in messages.iter().enumerate() {
        let start = Instant::now();
        let result = mirror.on_message(msg);
        let elapsed = start.elapsed().as_nanos() as u64;
        let result = black_box(result);

        match result {
            Ok(()) => match index {
                0 => snapshot_nanos = Some(elapsed),
                _ => update_nanos.push(elapsed),
            },
            Err(err) => {
                return Err(MeasureError {
                    index,
                    message: err.to_string(),
                    partial: Report {
                        snapshot_nanos,
                        update_nanos,
                        book_present: mirror.get_order_book().is_some(),
                    },
                });
            }
        }
    }

    Ok(Report {
        snapshot_nanos,
        update_nanos,
        book_present: black_box(mirror.get_order_book()).is_some(),
    })
}

// Sum every recorded sample so the apply loop stays live under Divan and CodSpeed.
pub fn replay(messages: &[String], depth: usize) -> Result<u128, Box<dyn Error>> {
    match measure(messages, depth) {
        Ok(report) => {
            let sum: u128 = report.update_nanos.iter().map(|nanos| *nanos as u128).sum();
            Ok(sum + u128::from(report.book_present))
        }
        Err(err) => Err(format!("error at message {}: {}", err.index, err.message).into()),
    }
}

// Nearest rank on a sorted slice: ceil(p/100 * n) - 1.
// `p` is a percent. 99.9 is scaled to per-mille so the rank stays in integers.
fn percentile(sorted_nanos: &[u64], p: f64) -> u64 {
    let n = sorted_nanos.len();
    if n == 0 {
        return 0;
    }
    let per_mille = (p * 10.0).round() as u128;
    let rank = (per_mille * n as u128).div_ceil(1000);
    let idx = (rank as usize).saturating_sub(1).min(n - 1);
    sorted_nanos[idx]
}

fn print_report(path: &Path, depth: usize, report: &Report) {
    let mut updates = report.update_nanos.clone();
    updates.sort_unstable();

    let applied = report.snapshot_nanos.is_some() as usize + updates.len();
    row("file", &path.display().to_string());
    row("depth", &depth.to_string());
    row("messages", &applied.to_string());
    row("snapshot", &format_sample(report.snapshot_nanos));
    row("updates", &updates.len().to_string());

    if updates.is_empty() {
        row("total", &format_us(0u64));
        row("throughput", "n/a");
        for label in ["min", "mean", "p50", "p90", "p95", "p99", "p99.9", "max"] {
            row(label, "n/a");
        }
        row(
            "book",
            if report.book_present {
                "present"
            } else {
                "missing"
            },
        );
        return;
    }

    let sum: u128 = updates.iter().map(|nanos| *nanos as u128).sum();
    let mean = ((sum + updates.len() as u128 / 2) / updates.len() as u128) as u64;
    row("total", &format_us(sum));
    row("throughput", &format_throughput(updates.len(), sum));
    row("min", &format_us(updates[0]));
    row("mean", &format_us(mean));
    row("p50", &format_us(percentile(&updates, 50.0)));
    row("p90", &format_us(percentile(&updates, 90.0)));
    row("p95", &format_us(percentile(&updates, 95.0)));
    row("p99", &format_us(percentile(&updates, 99.0)));
    row("p99.9", &format_us(percentile(&updates, 99.9)));
    row("max", &format_us(updates[updates.len() - 1]));
    row(
        "book",
        if report.book_present {
            "present"
        } else {
            "missing"
        },
    );
}

fn row(label: &str, value: &str) {
    println!("{label:<12}{value}");
}

fn format_sample(nanos: Option<u64>) -> String {
    match nanos {
        Some(nanos) => format_us(nanos),
        None => "n/a".to_string(),
    }
}

fn format_us(nanos: impl Into<u128>) -> String {
    format!("{:.3} µs", nanos.into() as f64 / 1_000.0)
}

fn format_throughput(count: usize, sum_nanos: u128) -> String {
    if sum_nanos == 0 {
        return "n/a".to_string();
    }
    let per_sec = count as f64 / (sum_nanos as f64 / 1_000_000_000.0);
    format!("{per_sec:.1} msg/s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_nearest_rank() {
        let samples = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(percentile(&samples, 50.0), 50);
        assert_eq!(percentile(&samples, 99.0), 100);
        assert_eq!(percentile(&[42], 50.0), 42);
        assert_eq!(percentile(&[42], 99.0), 42);
    }

    #[test]
    fn replay_empty_is_zero() {
        assert_eq!(replay(&[], 100).unwrap(), 0);
    }
}
