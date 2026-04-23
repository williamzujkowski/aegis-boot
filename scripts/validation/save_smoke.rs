// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Standalone smoke test for persistence::save_durable's write protocol
// against a real AEGIS_ISOS mount. Replicates the exact write sequence
// from crates/rescue-tui/src/persistence.rs:184 (atomic_write):
//
//   1. Write `.tmp` file with new content
//   2. Rename `.tmp` over the final path (atomic on Linux + exfat.ko >= 5.7)
//   3. Open the parent directory and `sync_all()` (dir fsync — flushes
//      the rename to flash per ADR 0003 §6.2)
//
// Compile: `rustc save_smoke.rs -O -o save_smoke`
//
// Usage:
//   AEGIS_ISOS_MOUNT=/mnt/aegis-isos ITERS=100 ./save_smoke
//
// Two modes:
//   - Happy-path throughput: run to completion, report per-iter timing.
//   - Kill-mid-save durability: launch in background with high ITERS
//     (e.g. 10000), then SIGKILL at a random offset; verify final state.
//
// See scripts/validation/README.md for full runbook.

fn main() -> std::io::Result<()> {
    let mount = std::env::var("AEGIS_ISOS_MOUNT")
        .expect("AEGIS_ISOS_MOUNT env var required (path to mounted AEGIS_ISOS)");
    let iters: usize = std::env::var("ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let state_dir = std::path::PathBuf::from(&mount).join(".aegis-state");
    std::fs::create_dir_all(&state_dir)?;
    let final_path = state_dir.join("last-choice.json");
    let tmp_path = state_dir.join("last-choice.json.tmp");

    println!(
        "save_smoke: {iters} iterations against {}",
        state_dir.display()
    );
    let t0 = std::time::Instant::now();
    for i in 0..iters {
        let body = format!(
            r#"{{
  "iso_path": "/run/media/aegis-isos/iter-{i}.iso",
  "cmdline_override": null
}}"#
        );
        std::fs::write(&tmp_path, &body)?;
        std::fs::rename(&tmp_path, &final_path)?;
        let dir_handle = std::fs::File::open(&state_dir)?;
        dir_handle.sync_all()?;
    }
    let elapsed = t0.elapsed();
    println!(
        "save_smoke: {iters} iters in {:?} ({:?}/iter)",
        elapsed,
        elapsed / iters as u32
    );

    let body = std::fs::read_to_string(&final_path)?;
    assert!(body.contains(&format!("iter-{}.iso", iters - 1)));
    assert!(!tmp_path.exists(), "tmp file survived — atomic_write leaked");
    println!("save_smoke: final file intact, no stale .tmp");
    Ok(())
}
