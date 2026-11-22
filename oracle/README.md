# The Dolos oracle

The ledger judge, `serve::grpc::v1::ledger_oracle_tests::ledger_oracle`, compares the answers of Dolos with those of the Haskell ledger on the corpora in `test_data/ledger-oracle`. It and its self tests are medium tests, so the test job runs them on every push and the image is built only when that job passes. `cargo nextest run -p dolos --lib --no-capture -E 'test(=serve::grpc::v1::ledger_oracle_tests::ledger_oracle)'` runs the judge alone, and each manifest line it proposes is printed after `CANDIDATE` and the name of its file.

`hs` holds the Haskell programs that write the corpora, and `probe` holds Python readers of a corpus that share no code with the judge.
