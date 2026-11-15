// WatchMempool at each RPC version runs these assertions.

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
