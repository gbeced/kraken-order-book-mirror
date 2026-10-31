use crate::kraken::rest_client;
use std::error::Error;
use std::fs::File;
use std::hint::black_box;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::kraken::level3::mirror;

#[derive(Debug)]
struct Report {
    update_nanos: Vec<u64>,
    book_present: bool,
}

#[derive(Debug)]
struct MeasureError {
    index: usize,
    message: String,
    partial: Report,
}

#[tokio::main]
pub async fn run(path: String, pair: String, depth: usize) -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        eprintln!(
            "warning: debug build timings are not comparable; use cargo run --release -- bench <file>"
        );
    }

    let path = PathBuf::from(path);
    let messages = load_messages(&path)?;
    if messages.is_empty() {
        return Err(format!("no messages in {}", path.display()).into());
    }
    let pair_info = rest_client::get_pair_info(&pair).await?;

    match measure(
        &messages,
        depth,
        pair_info.quote_precision,
        pair_info.base_precision,
    ) {
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

fn measure(
    messages: &[String],
    depth: usize,
    price_scale: u8,
    qty_scale: u8,
) -> Result<Report, MeasureError> {
    let mut mirror = mirror::OrderBookMirror::new(depth, price_scale, qty_scale);

    let mut update_nanos = Vec::with_capacity(messages.len());

    for (index, msg) in messages.iter().enumerate() {
        let start = Instant::now();
        let result = mirror.on_message(msg);
        let elapsed = start.elapsed().as_nanos() as u64;
        let result = black_box(result);

        match result {
            // Message 0 is the snapshot. Later frames are updates.
            Ok(()) => {
                if index != 0 {
                    update_nanos.push(elapsed);
                }
            }
            Err(err) => {
                return Err(MeasureError {
                    index,
                    message: err.to_string(),
                    partial: Report {
                        update_nanos,
                        book_present: mirror.get_order_book().is_some(),
                    },
                });
            }
        }
    }

    Ok(Report {
        update_nanos,
        book_present: black_box(mirror.get_order_book()).is_some(),
    })
}

// Sum of per-update nanos, plus one when the book is still present.
pub fn replay(
    messages: &[String],
    depth: usize,
    price_scale: u8,
    qty_scale: u8,
) -> Result<u128, Box<dyn Error>> {
    match measure(messages, depth, price_scale, qty_scale) {
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

    row("file", &path.display().to_string());
    row("depth", &depth.to_string());
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
        assert_eq!(replay(&[], 100, 1, 8).unwrap(), 0);
    }
}
