use dolos_core::{config::CardanoConfig, Domain, Genesis};
use dolos_testing::toy_domain::ToyDomain;
use std::{path::PathBuf, sync::Arc};

#[test]
fn musashi_bootstrap_uses_the_published_cost_models() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_data/musashi-cost-models");
    let genesis = Genesis::from_file_paths(
        root.join("byron-genesis.json"),
        root.join("shelley-genesis.json"),
        root.join("alonzo-genesis.json"),
        root.join("conway-genesis.json"),
        Some(12),
    )
    .unwrap()
    .with_dijkstra(root.join("dijkstra-genesis.json"))
    .unwrap();
    // Upstream carries the injected models through era translation. Verify
    // that path and actual bootstrap against the same published coefficients.
    let translated = dolos_cardano::forks::force_pparams_version(
        &dolos_cardano::forks::from_byron_genesis(&genesis.byron),
        &genesis,
        0,
        12,
    )
    .unwrap();
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("alonzo-genesis.json")).unwrap()).unwrap();
    let v3: Vec<i64> =
        serde_json::from_value(expected["extraConfig"]["costModels"]["PlutusV3"].clone()).unwrap();
    assert_eq!(translated.cost_models_plutus_v3().unwrap(), v3);
    let domain = ToyDomain::new_with_genesis_and_config(
        Arc::new(genesis),
        CardanoConfig {
            magic: 164,
            is_testnet: true,
            stop_epoch: None,
            custom_utxos: vec![],
        },
        None,
        None,
    );
    let epoch = dolos_cardano::load_epoch::<ToyDomain>(domain.state()).unwrap();
    let actual = epoch.pparams.unwrap_live();
    assert_eq!(
        actual.cost_models_plutus_v3().unwrap(),
        v3,
        "forced Musashi bootstrap must retain every published V3 override coefficient"
    );
    for (language, actual) in [
        ("PlutusV1", actual.cost_models_plutus_v1().unwrap()),
        ("PlutusV2", actual.cost_models_plutus_v2().unwrap()),
    ] {
        let expected: Vec<i64> =
            serde_json::from_value(expected["extraConfig"]["costModels"][language].clone())
                .unwrap();
        assert_eq!(
            actual, expected,
            "{language} override must be preserved exactly"
        );
    }
    let dijkstra: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("dijkstra-genesis.json")).unwrap())
            .unwrap();
    let v4: Vec<i64> = serde_json::from_value(dijkstra["plutusV4CostModel"].clone()).unwrap();
    assert_eq!(actual.cost_models_unknown_or_default().get(&3), Some(&v4));
}
