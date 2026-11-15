// WatchTx and WatchMempool at each RPC version run these assertions.

/// A transaction paying one address and holding the synthetic asset, as a block
/// of its own and as a mempool transaction.
struct Paid {
    block: RawBlock,
    tx: MempoolTx,
    address: Vec<u8>,
    elsewhere: Vec<u8>,
    policy: Vec<u8>,
    name: Vec<u8>,
}

/// The synthetic transaction with two outputs, one at its address and one at
/// another address, the asset held at its address when `together` and at the
/// other address otherwise.
fn paid(together: bool) -> Paid {
    use pallas::ledger::primitives::conway::{
        Block, PostAlonzoTransactionOutput, TransactionOutput, Value,
    };

    let (blocks, _, _) = dolos_testing::synthetic::build_synthetic_blocks(
        dolos_testing::synthetic::SyntheticBlockConfig {
            block_count: 1,
            txs_per_block: 1,
            ..Default::default()
        },
    );
    let (era, mut block): (u16, Block) = minicbor::decode(&blocks[0]).unwrap();
    let body = &mut block.transaction_bodies[0];
    let TransactionOutput::PostAlonzo(first) = &body.outputs[0] else {
        panic!("the synthetic output is a post Alonzo output");
    };
    let address = first.address.to_vec();
    let Value::Multiasset(coin, assets) = first.value.clone() else {
        panic!("the synthetic output holds the synthetic asset");
    };
    let (policy, names) = assets.iter().next().unwrap();
    let (policy, name) = (policy.to_vec(), names.keys().next().unwrap().to_vec());
    let mut elsewhere = address.clone();
    elsewhere[1..29].fill(0xbb);

    let output = |address: &[u8], value| {
        TransactionOutput::PostAlonzo(
            PostAlonzoTransactionOutput {
                address: address.to_vec().into(),
                value,
                datum_option: None,
                script_ref: None,
            }
            .into(),
        )
    };
    let (at_address, at_elsewhere) = if together {
        (Value::Multiasset(coin, assets), Value::Coin(coin))
    } else {
        (Value::Coin(coin), Value::Multiasset(coin, assets))
    };
    body.outputs = vec![output(&address, at_address), output(&elsewhere, at_elsewhere)];

    let raw = minicbor::to_vec((era, block)).unwrap();
    let decoded = pallas::ledger::traverse::MultiEraBlock::decode(&raw).unwrap();
    let tx = decoded.txs().remove(0);
    let tx = MempoolTx::new(tx.hash(), EraCbor(tx.era().into(), tx.encode()), vec![]);

    Paid {
        block: std::sync::Arc::new(raw),
        tx,
        address,
        elsewhere,
        policy,
        name,
    }
}

/// The pattern of an output at the paid address holding the synthetic asset.
// The pattern fields are optional at v1 and v1beta and plain at v1alpha.
#[allow(clippy::useless_conversion)]
fn holding_at_address(paid: &Paid) -> u5c::cardano::TxOutputPattern {
    u5c::cardano::TxOutputPattern {
        address: Some(u5c::cardano::AddressPattern {
            exact_address: bytes::Bytes::from(paid.address.clone()).into(),
            ..Default::default()
        }),
        asset: Some(u5c::cardano::AssetPattern {
            policy_id: bytes::Bytes::from(paid.policy.clone()).into(),
            asset_name: bytes::Bytes::from(paid.name.clone()).into(),
        }),
    }
}

/// The pattern of the paid address with the payment part of `payment_of`.
#[allow(clippy::useless_conversion)]
fn at_address_paying_to(paid: &Paid, payment_of: &[u8]) -> u5c::cardano::AddressPattern {
    u5c::cardano::AddressPattern {
        exact_address: bytes::Bytes::from(paid.address.clone()).into(),
        payment_part: bytes::Bytes::from(payment_of[1..29].to_vec()).into(),
        ..Default::default()
    }
}

/// The references WatchTx answers for `block` over `context` under `pattern`.
fn watch_tx_refs(
    block: &RawBlock,
    context: &impl ChainReads,
    pattern: u5c::cardano::TxPattern,
) -> Vec<Vec<u8>> {
    let request = u5c::watch::WatchTxRequest {
        predicate: Some(u5c::watch::TxPredicate {
            r#match: Some(u5c::watch::AnyChainTxPattern {
                chain: Some(u5c::watch::any_chain_tx_pattern::Chain::Cardano(pattern)),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    super::super::watch::block_to_txs(block, context, &request)
        .map(|x| match x.chain {
            Some(u5c::watch::any_chain_tx::Chain::Cardano(tx)) => tx_hash(&tx),
            None => panic!("an unmasked answer holds its transaction"),
        })
        .collect()
}

/// The references WatchMempool answers over `toy` for `tx` under `pattern`.
async fn watch_mempool_refs(
    toy: ToyDomain,
    tx: &MempoolTx,
    pattern: u5c::cardano::TxPattern,
) -> Vec<Vec<u8>> {
    let predicate = TxPredicate {
        r#match: Some(AnyChainTxPattern {
            chain: Some(any_chain_tx_pattern::Chain::Cardano(pattern)),
        }),
        ..Default::default()
    };
    let request = watch_request(Some(predicate), &["tx.ref"]);
    let (answers, _) = watched_in(toy, request, std::slice::from_ref(tx)).await;
    refs(&answers)
}

/// The references WatchTx and then WatchMempool answer for `paid` under
/// `pattern`.
async fn watch_answers(paid: &Paid, pattern: u5c::cardano::TxPattern) -> [Vec<Vec<u8>>; 2] {
    [
        watch_tx_refs(&paid.block, &ToyDomain::new(None, None), pattern.clone()),
        watch_mempool_refs(ToyDomain::new(None, None), &paid.tx, pattern).await,
    ]
}

/// A part of a transaction, besides its spent inputs and its outputs, that an
/// address or asset pattern covers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Collateral,
    CollateralReturn,
    ReferenceInput,
}

/// A child of `parent` that spends and pays only the other address, and names
/// the output at the paid address, or pays that address, in `part` alone, as a
/// block of its own and as a mempool transaction.
fn child_touching(parent: &Paid, part: Option<Part>) -> (RawBlock, MempoolTx) {
    use pallas::codec::utils::NonEmptySet;
    use pallas::ledger::primitives::conway::{Block, TransactionInput};

    let (era, mut block): (u16, Block) = minicbor::decode(&parent.block).unwrap();
    let body = &mut block.transaction_bodies[0];
    let [at_address, at_elsewhere] = [0, 1].map(|i| body.outputs[i].clone());
    let output = |index| TransactionInput {
        transaction_id: parent.tx.hash(),
        index,
    };
    let named_in = |covering| if part == Some(covering) { 0 } else { 1 };

    body.inputs = vec![output(1)].into();
    body.outputs = vec![at_elsewhere.clone()];
    body.mint = None;
    body.collateral = NonEmptySet::from_vec(vec![output(named_in(Part::Collateral))]);
    body.collateral_return = Some(if part == Some(Part::CollateralReturn) {
        at_address
    } else {
        at_elsewhere
    });
    body.reference_inputs = NonEmptySet::from_vec(vec![output(named_in(Part::ReferenceInput))]);

    let raw = minicbor::to_vec((era, block)).unwrap();
    let decoded = pallas::ledger::traverse::MultiEraBlock::decode(&raw).unwrap();
    let tx = decoded.txs().remove(0);
    let tx = MempoolTx::new(tx.hash(), EraCbor(tx.era().into(), tx.encode()), vec![]);
    (std::sync::Arc::new(raw), tx)
}

/// A domain whose ledger holds the outputs of `parent`.
fn ledger_holding_outputs_of(parent: &Paid) -> crate::adapters::DomainAdapter {
    let decoded = MultiEraTx::decode_for_era(Era::Conway, parent.tx.payload().cbor()).unwrap();
    let produced_utxo = decoded
        .outputs()
        .iter()
        .enumerate()
        .map(|(index, output)| {
            (
                TxoRef(decoded.hash(), index as TxoIdx),
                std::sync::Arc::new(EraCbor(output.era().into(), output.encode())),
            )
        })
        .collect();
    let toy = ToyDomain::new(
        Some(UtxoSetDelta {
            produced_utxo,
            ..Default::default()
        }),
        None,
    );
    crate::tests::toy_adapter(&toy)
}

/// A domain whose mempool holds `parent`.
fn mempool_holding(parent: &Paid) -> ToyDomain {
    let toy = ToyDomain::new(None, None);
    toy.mempool().receive(parent.tx.clone()).unwrap();
    toy
}

/// For each part, whether WatchTx, over a ledger holding the outputs of
/// `parent`, and then WatchMempool, over a mempool holding `parent`, answer
/// the child of `parent` touching it in that part under `pattern`.
async fn admitted_by_part(
    parent: &Paid,
    parts: &[Option<Part>],
    pattern: u5c::cardano::TxPattern,
) -> Vec<(Option<Part>, [bool; 2])> {
    let mut admitted = vec![];
    for &part in parts {
        let (block, tx) = child_touching(parent, part);
        let answered = |refs: Vec<Vec<u8>>| match refs.as_slice() {
            [] => false,
            [only] if *only == tx.hash().to_vec() => true,
            other => panic!("the answers name only the child: {other:?}"),
        };
        let watch_tx = watch_tx_refs(&block, &ledger_holding_outputs_of(parent), pattern.clone());
        let watch_mempool = watch_mempool_refs(mempool_holding(parent), &tx, pattern.clone()).await;
        admitted.push((part, [answered(watch_tx), answered(watch_mempool)]));
    }
    admitted
}

#[tokio::test]
async fn watch_methods_match_has_address_on_collateral_and_reference_inputs() {
    let parent = paid(true);
    let pattern = u5c::cardano::TxPattern {
        has_address: Some(at_address_paying_to(&parent, &parent.address)),
        ..Default::default()
    };
    let parts = [
        None,
        Some(Part::Collateral),
        Some(Part::CollateralReturn),
        Some(Part::ReferenceInput),
    ];
    let expected: Vec<_> = parts
        .iter()
        .map(|&part| (part, [part.is_some(); 2]))
        .collect();

    assert_eq!(admitted_by_part(&parent, &parts, pattern).await, expected);
}

#[tokio::test]
async fn watch_methods_match_moves_asset_on_collateral_but_not_a_reference_input() {
    let parent = paid(true);
    let pattern = u5c::cardano::TxPattern {
        moves_asset: holding_at_address(&parent).asset,
        ..Default::default()
    };
    let parts = [
        None,
        Some(Part::ReferenceInput),
        Some(Part::Collateral),
        Some(Part::CollateralReturn),
    ];
    let expected: Vec<_> = parts
        .iter()
        .map(|&part| {
            let moved = matches!(part, Some(Part::Collateral | Part::CollateralReturn));
            (part, [moved; 2])
        })
        .collect();

    assert_eq!(admitted_by_part(&parent, &parts, pattern).await, expected);
}

#[tokio::test]
async fn watch_methods_match_produces_only_on_one_output() {
    for (together, admitted) in [(false, false), (true, true)] {
        let paid = paid(together);
        let pattern = u5c::cardano::TxPattern {
            produces: Some(holding_at_address(&paid)),
            ..Default::default()
        };
        let expected = if admitted {
            vec![paid.tx.hash().to_vec()]
        } else {
            vec![]
        };

        assert_eq!(
            watch_answers(&paid, pattern).await,
            [expected.clone(), expected],
            "{together}"
        );
    }
}

#[tokio::test]
async fn watch_methods_match_has_address_only_on_one_address() {
    let paid = paid(true);
    for (payment_of, admitted) in [(&paid.elsewhere, false), (&paid.address, true)] {
        let pattern = u5c::cardano::TxPattern {
            has_address: Some(at_address_paying_to(&paid, payment_of)),
            ..Default::default()
        };
        let expected = if admitted {
            vec![paid.tx.hash().to_vec()]
        } else {
            vec![]
        };

        assert_eq!(
            watch_answers(&paid, pattern).await,
            [expected.clone(), expected],
            "{admitted}"
        );
    }
}

#[tokio::test]
async fn watch_mempool_matches_consumes_only_on_one_input() {
    for (together, admitted) in [(false, false), (true, true)] {
        let parent = paid(together);
        let child = spending(&parent.tx, &[0, 1]);
        let toy = ToyDomain::new(None, None);
        toy.mempool().receive(parent.tx.clone()).unwrap();
        let pattern = u5c::cardano::TxPattern {
            consumes: Some(holding_at_address(&parent)),
            ..Default::default()
        };
        let expected = if admitted {
            vec![child.hash().to_vec()]
        } else {
            vec![]
        };

        assert_eq!(
            watch_mempool_refs(toy, &child, pattern).await,
            expected,
            "{together}"
        );
    }
}

#[tokio::test]
async fn watch_mempool_answers_each_clause_as_the_spec_defines_it() {
    let txs = minting_txs();
    let [alpha, beta] = [0, 1].map(|i| txs[i].hash().to_vec());
    let any_of = |list: Vec<TxPredicate>| TxPredicate {
        any_of: list,
        ..Default::default()
    };
    let all_of = |list: Vec<TxPredicate>| TxPredicate {
        all_of: list,
        ..Default::default()
    };
    let none_of = |list: Vec<TxPredicate>| TxPredicate {
        not: list,
        ..Default::default()
    };

    for (predicate, expected) in [
        (TxPredicate::default(), vec![alpha.clone(), beta.clone()]),
        (any_of(vec![]), vec![alpha.clone(), beta.clone()]),
        (all_of(vec![]), vec![alpha.clone(), beta.clone()]),
        (all_of(vec![mints("alpha"), mints("beta")]), vec![]),
        (all_of(vec![mints("alpha")]), vec![alpha.clone()]),
        (
            any_of(vec![mints("alpha"), mints("beta")]),
            vec![alpha.clone(), beta.clone()],
        ),
        (any_of(vec![mints("gamma")]), vec![]),
        (none_of(vec![mints("alpha"), mints("beta")]), vec![]),
        (
            TxPredicate {
                not: vec![mints("alpha")],
                ..mints("alpha")
            },
            vec![],
        ),
        (
            all_of(vec![
                any_of(vec![mints("alpha"), mints("beta")]),
                none_of(vec![mints("beta")]),
            ]),
            vec![alpha.clone()],
        ),
        (
            any_of(vec![mints("alpha"), none_of(vec![mints("alpha")])]),
            vec![alpha.clone(), beta.clone()],
        ),
    ] {
        let (answers, _) = watched(watch_request(Some(predicate.clone()), &["tx.ref"]), &txs).await;
        assert_eq!(refs(&answers), expected, "{predicate:?}");
    }
}
