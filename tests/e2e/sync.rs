#![cfg(not(windows))]

use std::process::Stdio;
use std::time::Duration;

#[path = "common.rs"]
mod common;

use common::*;

fn daemon_syncs(workspace: &ScenarioWorkspace) {
    println!("e2e sync start: {}", workspace.name());

    reset_and_bootstrap(workspace);

    let mut cmd = prepare_scenario_process(workspace);

    let handle = cmd
        .args(["daemon"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn process");

    let mut guard = ProcessGuard::new(handle);

    std::thread::sleep(Duration::from_secs(60));

    assert!(guard
        .try_wait()
        .expect("failed to query process status")
        .is_none());

    shutdown_gracefully(&mut guard);

    let after = fetch_summary(workspace);

    let after_tip = after
        .wal
        .tip_slot
        .into_iter()
        .chain(after.archive.tip_slot)
        .chain(after.state.tip_slot)
        .max()
        .unwrap_or_default();

    // A relay bootstrap writes no store, so the daemon starts from no tip.
    assert!(
        after_tip > 0,
        "expected tip to advance slots for {}, after={after_tip}",
        workspace.name()
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

test_for_scenario!(daemon_syncs_for_preview_full_explicit, daemon_syncs, 0);
test_for_scenario!(daemon_syncs_for_preview_full_implicit, daemon_syncs, 1);
test_for_scenario!(daemon_syncs_for_preview_min_implicit, daemon_syncs, 2);
test_for_scenario!(daemon_syncs_for_mainnet_full_implicit, daemon_syncs, 3);
test_for_scenario!(daemon_syncs_for_preprod_full_implicit, daemon_syncs, 4);
test_for_scenario!(daemon_syncs_for_custom_network, daemon_syncs, 5);
