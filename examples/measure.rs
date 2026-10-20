//! Reproducible data-layer timings. Use an empty cache for a first-index run:
//! cargo run --release --example measure -- INPUT CACHE_DIRECTORY
//! For process peak RSS, run the built example under /usr/bin/time -l (macOS).

use seqviz::data::Dataset;
use std::{
    env, fs, io,
    path::PathBuf,
    time::{Duration, Instant},
};

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage: measure INPUT CACHE_DIRECTORY",
        ));
    }
    let path = PathBuf::from(&args[0]);
    let cache = PathBuf::from(&args[1]);
    let start = Instant::now();
    let mut dataset = Dataset::open(&path, &cache)?;
    let opened = start.elapsed();
    let preindexed = dataset.progress().complete;
    let mut first_window = None;
    let mut first_window_len = 0;
    loop {
        let progress = dataset.progress();
        if let Some(error) = progress.error {
            return Err(io::Error::other(error));
        }
        if first_window.is_none()
            && let Some(meta) = dataset.record(0)?
            && (meta.length >= 1920 || meta.complete)
        {
            first_window_len = dataset.read_range(0, 0, 1920)?.sequence.len();
            first_window = Some(start.elapsed());
        }
        if progress.complete {
            break;
        }
        dataset.step(64 * 1024)?;
    }
    let scanned = start.elapsed();
    let progress = dataset.progress();
    let first_window = first_window.unwrap_or(scanned);
    println!("metric\tvalue");
    println!("path\t{}", path.display());
    println!("input_bytes\t{}", fs::metadata(&path)?.len());
    println!("format\t{}", dataset.format().name());
    println!("compression\t{}", dataset.compression());
    println!("cache_complete_on_open\t{preindexed}");
    println!("open_ms\t{:.3}", milliseconds(opened));
    println!("first_window_ms\t{:.3}", milliseconds(first_window));
    println!("first_window_bases\t{first_window_len}");
    println!("complete_scan_ms\t{:.3}", milliseconds(scanned));
    println!("records\t{}", progress.records);
    println!("logical_bytes\t{}", progress.scanned_bytes);
    println!("read_errors\t{}", progress.errors);
    if progress.errors != 0 {
        return Err(io::Error::other("Measurement input contains read errors"));
    }
    let stats_start = Instant::now();
    let stats = dataset.stats()?;
    println!("statistics_ms\t{:.3}", milliseconds(stats_start.elapsed()));
    println!("total_sequence_bases\t{}", stats.total_length);
    drop(dataset);

    let reopen_start = Instant::now();
    let mut reopened = Dataset::open(&path, &cache)?;
    println!(
        "indexed_reopen_ms\t{:.3}",
        milliseconds(reopen_start.elapsed())
    );
    if !reopened.progress().complete {
        return Err(io::Error::other("Completed index was not reused"));
    }
    let window_start = Instant::now();
    let _ = reopened.read_range(0, 0, 1920)?;
    println!(
        "indexed_first_window_ms\t{:.3}",
        milliseconds(window_start.elapsed())
    );

    // Alternate distant and backward ranges; each includes record lookup and I/O.
    let fractions = [99, 0, 51, 3, 80, 20, 98, 1];
    let mut latencies = Vec::new();
    let mut checksum = 0u64;
    for _ in 0..3 {
        for fraction in fractions {
            if progress.records == 0 {
                continue;
            }
            let before = Instant::now();
            let id = (progress.records - 1) * fraction / 100;
            let meta = reopened
                .record(id)?
                .ok_or_else(|| io::Error::other("Indexed record is missing"))?;
            let from = if progress.records == 1 {
                meta.length.saturating_sub(1) * fraction / 100
            } else {
                0
            };
            let window = reopened.read_range(id, from, 4096)?;
            let expected_len = (meta.length.saturating_sub(from)).min(4096) as usize;
            if window.sequence.len() != expected_len {
                return Err(io::Error::other("Range length disagrees with record index"));
            }
            latencies.push(milliseconds(before.elapsed()));
            for byte in window.sequence.iter().chain(&window.quality) {
                checksum = checksum.wrapping_mul(31).wrapping_add(u64::from(*byte));
            }
        }
    }
    latencies.sort_by(f64::total_cmp);
    println!("range_samples\t{}", latencies.len());
    if !latencies.is_empty() {
        println!("range_median_ms\t{:.3}", latencies[latencies.len() / 2]);
        println!(
            "range_p95_ms\t{:.3}",
            latencies[(latencies.len() * 95).div_ceil(100) - 1]
        );
        println!("range_max_ms\t{:.3}", latencies[latencies.len() - 1]);
    }
    println!("range_checksum\t{checksum}");
    Ok(())
}
