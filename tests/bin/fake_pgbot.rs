//! Deterministic stand-in for the pgbot CLI. Behavior keys off the DSN in
//! $DATABASE_URL; scratch state lives beside the executable, which
//! `write_fake_pgbot` copies into each test's temp directory.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn scratch_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn append_line(path: &Path, line: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        // One write_all, newline included: concurrent fakes append to these
        // files, and a split write interleaves into "22\n\n".
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

fn emit(dir: &Path, fixture: &str, code: i32) -> ! {
    match std::fs::read_to_string(Path::new(FIXTURES).join(fixture)) {
        Ok(body) => print!("{body}"),
        Err(e) => {
            eprintln!("fake-pgbot: cannot read fixture {fixture}: {e}");
            finish(dir, 64);
        }
    }
    let _ = std::io::stdout().flush();
    finish(dir, code)
}

fn finish(dir: &Path, code: i32) -> ! {
    let _ = std::fs::remove_file(dir.join(format!("running.{}", std::process::id())));
    std::process::exit(code)
}

fn delay() {
    let secs = std::env::var("FAKE_PGBOT_DELAY")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    if secs > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }
}

fn live_markers(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("running."))
        .count()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if matches!(args.first().map(String::as_str), Some("--version" | "-v")) {
        println!("pgbot version 0.9.9");
        return;
    }

    let dir = scratch_dir();
    let dsn = std::env::var("DATABASE_URL").unwrap_or_default();

    append_line(
        &dir.join("invocations.log"),
        &format!("{} url={dsn}", args.join(" ")),
    );

    let _ = std::fs::write(dir.join(format!("running.{}", std::process::id())), b"");
    append_line(&dir.join("peaks.log"), &live_markers(&dir).to_string());

    match args.first().map(String::as_str) {
        Some("indexes") => emit(&dir, "indexes_report.json", 0),
        Some("why") => emit(&dir, "why_report.json", 0),
        _ => {}
    }

    if dsn.contains("mode-healthy") {
        delay();
        emit(&dir, "context_healthy.json", 0);
    } else if dsn.contains("mode-warn") {
        delay();
        emit(&dir, "context_warn.json", 1);
    } else if dsn.contains("mode-critical") {
        emit(&dir, "context_critical.json", 2);
    } else if dsn.contains("mode-refuse") {
        eprintln!(
            "pgbot: connect postgres://alex:sekret-pw@db.internal:5432/app: connection refused"
        );
        finish(&dir, 3);
    } else if dsn.contains("mode-hang") {
        std::thread::sleep(Duration::from_secs(60));
        finish(&dir, 0);
    } else {
        eprintln!("pgbot: no connection string (pass one or set $DATABASE_URL)");
        finish(&dir, 3);
    }
}
