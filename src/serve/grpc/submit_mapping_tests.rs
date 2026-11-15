// Synthetic reports exercise the versioned protobuf mappings, including logs
// unavailable from the currently supported native Trace builtin subset.
use super::*;
use pallas::ledger::{
    primitives::ExUnits as PallasUnits, traverse::MultiEraRedeemerTag as RedeemerTag,
    validate::phase2::tx::MultiEraTxEvalResult,
};

fn entry(tag: RedeemerTag, index: u32, success: bool) -> MultiEraTxEvalResult {
    MultiEraTxEvalResult {
        sub_transaction: None,
        tag,
        index,
        success,
        units: PallasUnits {
            mem: 18485,
            steps: 4805428,
        },
        logs: vec![],
        failure_message: None,
    }
}

fn evaluation(
    report: Vec<MultiEraTxEvalResult>,
) -> crate::serve::grpc::eval::TxEvaluation<u5c::cardano::Redeemer> {
    crate::serve::grpc::eval::TxEvaluation {
        report,
        min_fee: None,
        redeemers: vec![],
    }
}

/// The version's answer for an evaluation that holds mapped redeemers.
fn tx_eval_to_u5c(
    eval: Result<crate::serve::grpc::eval::TxEvaluation<u5c::cardano::Redeemer>, DomainError>,
) -> u5c::cardano::TxEval {
    super::tx_eval_to_u5c(eval, Clone::clone)
}

#[test]
fn the_fee_is_the_minimum_fee_and_unset_without_one() {
    use u5c::cardano::{big_int, BigInt};
    let unpriced = tx_eval_to_u5c(Ok(evaluation(vec![entry(RedeemerTag::Spend, 0, true)])));
    let mut priced = evaluation(vec![entry(RedeemerTag::Spend, 0, true)]);
    priced.min_fee = Some(165413);

    let priced = tx_eval_to_u5c(Ok(priced));

    assert_eq!(unpriced.fee, None);
    assert_eq!(
        priced.fee,
        Some(BigInt {
            big_int: Some(big_int::BigInt::Int(165413)),
        })
    );
}

#[test]
fn a_row_carries_the_data_of_the_redeemer_under_its_tag_and_index() {
    use u5c::cardano::{plutus_data, PlutusData, Redeemer};
    let held = |tag, index, byte: u8| {
        let redeemer = Redeemer {
            payload: Some(PlutusData {
                plutus_data: Some(plutus_data::PlutusData::BoundedBytes(vec![byte].into())),
            }),
            original_cbor: vec![byte].into(),
            ..Default::default()
        };
        (tag, index, redeemer)
    };
    let mut held_by = evaluation(vec![
        entry(RedeemerTag::Spend, 0, true),
        entry(RedeemerTag::Cert, 1, true),
    ]);
    held_by.redeemers = vec![
        held(RedeemerTag::Mint, 0, 1),
        held(RedeemerTag::Spend, 0, 2),
        held(RedeemerTag::Cert, 1, 3),
    ];

    let report = tx_eval_to_u5c(Ok(held_by));

    let rows = report
        .redeemers
        .iter()
        .map(|r| {
            (
                r.original_cbor.to_vec(),
                r.payload.clone(),
                r.ex_units.clone(),
            )
        })
        .collect::<Vec<_>>();
    let expected = [2, 3].map(|byte| {
        let (_, _, redeemer) = held(RedeemerTag::Spend, 0, byte);
        (
            vec![byte],
            redeemer.payload,
            Some(ExUnits {
                memory: 18485,
                steps: 4805428,
            }),
        )
    });
    assert_eq!(rows, expected);
}

#[test]
fn estimation_mapping_all_purposes_and_total() {
    use u5c::cardano::RedeemerPurpose as P;
    let tags = [
        (RedeemerTag::Spend, P::Spend),
        (RedeemerTag::Mint, P::Mint),
        (RedeemerTag::Cert, P::Cert),
        (RedeemerTag::Reward, P::Reward),
        (RedeemerTag::Vote, P::Vote),
        (RedeemerTag::Propose, P::Propose),
    ];
    let report = tx_eval_to_u5c(Ok(evaluation(
        tags.iter()
            .enumerate()
            .map(|(i, (tag, _))| entry(*tag, i as u32 + 2, true))
            .collect(),
    )));
    assert!(report.errors.is_empty());
    for (i, (_, purpose)) in tags.iter().enumerate() {
        assert_eq!(report.redeemers[i].purpose, *purpose as i32);
        assert_eq!(report.redeemers[i].index, i as u32 + 2);
    }
    assert_eq!(
        report.ex_units,
        Some(ExUnits {
            memory: 6 * 18485,
            steps: 6 * 4805428
        })
    );
}

#[test]
fn estimation_mapping_failures_traces_and_partial_success() {
    let mut failed = entry(RedeemerTag::Spend, 3, false);
    failed.failure_message = Some("Explicit error".into());
    failed.logs = vec!["before failure".into(), "detail".into()];
    let fallback = entry(RedeemerTag::Mint, 0, false);
    let mut success = entry(RedeemerTag::Cert, 2, true);
    success.logs = vec!["completed".into()];
    let report = tx_eval_to_u5c(Ok(evaluation(vec![failed, fallback, success])));
    assert_eq!(report.redeemers.len(), 3);
    assert_eq!(report.errors.len(), 2);
    assert_eq!(report.errors[0].msg, "Spend[3]: Explicit error");
    assert_eq!(report.errors[1].msg, "Mint[0]: script evaluation failed");
    assert_eq!(
        report
            .traces
            .iter()
            .map(|x| x.msg.as_str())
            .collect::<Vec<_>>(),
        ["before failure", "detail", "completed"]
    );
    assert_eq!(
        report.ex_units,
        Some(ExUnits {
            memory: 3 * 18485,
            steps: 3 * 4805428
        })
    );
}

#[test]
fn sub_transaction_results_are_explicitly_rejected() {
    for position in 0..3 {
        let mut sub = entry(RedeemerTag::Spend, 4, true);
        sub.sub_transaction = Some(1);
        sub.logs = vec!["sub log".into()];
        let mut entries = vec![
            entry(RedeemerTag::Spend, 4, true),
            entry(RedeemerTag::Cert, 2, false),
        ];
        entries.insert(position, sub);
        let report = tx_eval_to_u5c(Ok(evaluation(entries)));
        let [error] = &report.errors[..] else {
            panic!("{:?}", report.errors)
        };
        let (version, rest) = error.msg.split_once(' ').unwrap();
        assert_eq!(version, "RPC");
        assert!(
            rest.ends_with(" cannot represent redeemer Spend[4] of sub transaction 1"),
            "{error:?}"
        );
        assert!(report.redeemers.is_empty());
        assert!(report.traces.is_empty());
        assert!(report.ex_units.is_none());
    }
}

#[test]
fn estimation_mapping_empty_and_overflow() {
    let empty = tx_eval_to_u5c(Ok(evaluation(vec![])));
    assert_eq!(empty.ex_units, Some(ExUnits::default()));
    assert!(empty.errors.is_empty());
    for memory in [false, true] {
        let mut huge = entry(RedeemerTag::Spend, 0, true);
        if memory {
            huge.units.mem = u64::MAX;
        } else {
            huge.units.steps = u64::MAX;
        }
        let report = tx_eval_to_u5c(Ok(evaluation(vec![
            huge,
            entry(RedeemerTag::Mint, 0, true),
        ])));
        assert!(report.ex_units.is_none());
        assert_eq!(report.redeemers.len(), 2);
        assert_eq!(report.errors[0].msg, "total execution units overflow");
    }
}
