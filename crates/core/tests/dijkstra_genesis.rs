//! Reading a Dijkstra genesis file.
//!
//! The file under test is the Musashi node's own, copied byte for byte, so a
//! value asserted here is a value the chain is being governed by. Its
//! provenance is beside it.

use std::path::{Path, PathBuf};

use dolos_core::{dijkstra, Genesis};

fn test_data() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("test_data")
}

fn musashi_dijkstra_path() -> PathBuf {
    test_data().join("musashi").join("dijkstra-genesis.json")
}

fn preview_genesis_paths(root: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    dolos_cardano::include::preview::save(root).unwrap();

    (
        root.join("byron.json"),
        root.join("shelley.json"),
        root.join("alonzo.json"),
        root.join("conway.json"),
    )
}

/// MUST FIRE: every parameter the node's file declares reaches the value the
/// file states, one assertion per parameter.
///
/// A count of parameters would pass on a file whose values were all read into
/// the wrong fields, and a struct comparison would say only that something
/// differs, so each one is named.
#[test]
fn every_parameter_of_the_node_genesis_is_read() {
    let file = dijkstra::from_file(musashi_dijkstra_path()).unwrap();

    assert_eq!(file.leios_announcement_period_length, 1000);
    assert_eq!(file.leios_committee_size, 900);
    assert_eq!(file.leios_diffusion_period_length, 7000);
    assert_eq!(file.leios_quorum_stake_threshold, 0.75);
    assert_eq!(file.leios_vote_period_length, 4000);
    assert_eq!(file.max_endorser_block_execution_units.memory, 310_000_000);
    assert_eq!(
        file.max_endorser_block_execution_units.steps,
        100_000_000_000
    );
    assert_eq!(file.max_endorser_block_references_size, 100_000);
    assert_eq!(file.max_endorser_block_txs_size, 1_000_000);
    assert_eq!(file.max_pledge_leverage, None);
    assert_eq!(file.max_ref_script_size_per_block, 1_048_576);
    assert_eq!(file.max_ref_script_size_per_endorser_block, 4_000_000);
    assert_eq!(file.max_ref_script_size_per_tx, 204_800);
    assert_eq!(file.min_pool_margin, 0.015);
    assert_eq!(file.plutus_v4_cost_model.len(), 251);
    assert_eq!(file.plutus_v4_cost_model.first(), Some(&100_788));
    assert_eq!(
        file.plutus_v4_cost_model.iter().min(),
        Some(&-900),
        "a cost model entry is negative, so the entries are signed",
    );
    assert_eq!(file.ref_script_cost_multiplier, 1.2);
    assert_eq!(file.ref_script_cost_stride, 25_600);
}

/// MUST FIRE: a parameter the file names and this type does not model is
/// refused, because a follower running without it is applying a different
/// rule set from the node and cannot say so.
///
/// MUST NOT FIRE: the same file without the extra parameter loads, so the
/// refusal is about the unknown parameter and not about the file.
#[test]
fn a_parameter_with_nowhere_to_go_stops_the_load() {
    let original = std::fs::read_to_string(musashi_dijkstra_path()).unwrap();

    assert!(
        dijkstra::from_file(musashi_dijkstra_path()).is_ok(),
        "the file this case edits loads as it stands",
    );

    let widened = original.replacen('{', "{\"leiosSomethingNew\": 1,", 1);

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dijkstra-genesis.json");
    std::fs::write(&path, widened).unwrap();

    let error = dijkstra::from_file(&path).expect_err("an unknown parameter was accepted");

    assert!(
        error.to_string().contains("leiosSomethingNew"),
        "the error does not name the parameter it refused: {error}",
    );
}

/// MUST FIRE: a genesis loaded without a Dijkstra path carries no Dijkstra
/// parameters, and one loaded with the path carries them, so the field is
/// additive and a caller can tell the two apart.
#[test]
fn the_dijkstra_file_is_additive_to_the_genesis() {
    let directory = tempfile::tempdir().unwrap();
    let (byron, shelley, alonzo, conway) = preview_genesis_paths(directory.path());

    let without = Genesis::from_file_paths(&byron, &shelley, &alonzo, &conway, None).unwrap();

    assert!(
        without.dijkstra.is_none(),
        "a genesis with no dijkstra path invented one",
    );

    let with = Genesis::from_file_paths(&byron, &shelley, &alonzo, &conway, None)
        .unwrap()
        .with_dijkstra(musashi_dijkstra_path())
        .unwrap();

    let dijkstra = with.dijkstra.expect("the dijkstra file was not carried");

    assert_eq!(dijkstra.leios_committee_size, 900);
    assert_eq!(with.shelley_hash, without.shelley_hash);
}

/// MUST FIRE: a path that names no file is an error rather than an empty set
/// of parameters, because a missing consensus rule and a rule set of zero
/// rules are not the same answer.
#[test]
fn a_missing_dijkstra_file_is_an_error() {
    let directory = tempfile::tempdir().unwrap();

    let error = dijkstra::from_file(directory.path().join("absent.json"))
        .expect_err("a missing file produced a genesis");

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}
