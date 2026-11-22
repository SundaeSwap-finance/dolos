#![cfg(not(windows))]

use std::process::Stdio;

#[path = "common.rs"]
mod common;

use common::*;

fn daemon_runs(workspace: &ScenarioWorkspace) {
    println!("e2e smoke start: {}", workspace.name());

    assert_file_released(workspace, "dolos.socket");

    reset_and_bootstrap(workspace);

    let mut guard = workspace.spawn_daemon();

    wait_for_ports(workspace, &mut guard);
    if workspace.expect_ports() {
        wait_for_file(workspace, &mut guard, "dolos.socket");
    }
    wait_for_block(&mut guard);

    shutdown_gracefully(&mut guard);

    assert_ports_released(workspace);
    assert_file_released(workspace, "dolos.socket");

    // The custom network produces its own blocks, so it is the scenario that
    // can show a fresh node's first frames on disk without an upstream.
    if workspace.name() == "custom-network" {
        assert_archive_is_frames_and_reads_back(workspace);
    }
}

fn segment_files(workspace: &ScenarioWorkspace) -> Vec<std::path::PathBuf> {
    let archive = workspace.path().join("data").join("archive");
    let mut files: Vec<_> = match std::fs::read_dir(&archive) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "segment"))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files
}

/// Every byte the daemon left in its segment files is a zstd frame for the
/// bundled dictionary, with no compression setting or command anywhere in
/// the scenario, and a fresh process reads every one of them back.
fn assert_archive_is_frames_and_reads_back(workspace: &ScenarioWorkspace) {
    use dolos_fjall::flatfiles::{BUNDLED_DICTIONARY, MAX_BODY_BYTES};

    let files = segment_files(workspace);
    assert!(!files.is_empty(), "the daemon wrote no segment");
    let dictionary_id = zstd::zstd_safe::get_dict_id_from_dict(BUNDLED_DICTIONARY).unwrap();
    let mut decompressor = zstd::bulk::Decompressor::with_dictionary(BUNDLED_DICTIONARY).unwrap();
    let mut frames = 0usize;
    let mut raw = 0usize;
    let mut on_disk = 0usize;
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        on_disk += bytes.len();
        let mut cursor = 0;
        while cursor < bytes.len() {
            let size = zstd::zstd_safe::find_frame_compressed_size(&bytes[cursor..])
                .unwrap_or_else(|_| panic!("{}: not a frame at {cursor}", file.display()));
            let frame = &bytes[cursor..cursor + size];
            assert_eq!(
                zstd::zstd_safe::get_dict_id_from_frame(frame),
                Some(dictionary_id),
                "{}: frame at {cursor} was not written with the bundled dictionary",
                file.display()
            );
            raw += decompressor
                .decompress(frame, MAX_BODY_BYTES)
                .unwrap()
                .len();
            frames += 1;
            cursor += size;
        }
    }
    assert!(frames > 0);
    assert!(on_disk < raw, "{on_disk} bytes on disk for {raw} raw");

    // A second process opens the store from nothing but the directory and
    // lists every block the index names.
    let output = prepare_scenario_process(workspace)
        .args(["data", "dump-blocks"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to dump blocks");
    assert!(
        output.status.success(),
        "dump-blocks failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            line.split('|').any(|cell| {
                let cell = cell.trim();
                cell.len() == 64 && cell.bytes().all(|b| b.is_ascii_hexdigit())
            })
        })
        .count();
    assert_eq!(
        listed, frames,
        "the index names {listed} blocks but the segments hold {frames} frames"
    );
}

macro_rules! test_for_scenario {
    ($name:ident, $func:ident, $scenario:expr) => {
        #[test]
        fn $name() {
            $func(&ScenarioWorkspace::new(&SCENARIOS[$scenario]));
        }
    };
}

test_for_scenario!(daemon_runs_for_preview_full_explicit, daemon_runs, 0);
test_for_scenario!(daemon_runs_for_preview_full_implicit, daemon_runs, 1);
test_for_scenario!(daemon_runs_for_preview_min_implicit, daemon_runs, 2);
test_for_scenario!(daemon_runs_for_mainnet_full_implicit, daemon_runs, 3);
test_for_scenario!(daemon_runs_for_preprod_full_implicit, daemon_runs, 4);
test_for_scenario!(daemon_runs_for_custom_network, daemon_runs, 5);
