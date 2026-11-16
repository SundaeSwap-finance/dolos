# The Dolos oracle

The ledger judge, `serve::grpc::v1::ledger_oracle_tests::ledger_oracle`, compares the answers of Dolos with those of the Haskell ledger on the corpora in `test_data/ledger-oracle`. The `judge` job of `.github/workflows/docker.yml` runs it and its self tests on every `leios-musashi` push, and the image is built only when that job and the test job pass. `cargo test --release -p dolos --lib -- serve::grpc::v1::ledger_oracle_tests::ledger_oracle --exact --ignored --test-threads=1` runs the judge alone, and each manifest line it proposes is printed after `CANDIDATE` and the name of its file.

`hs` holds the Haskell programs that write the corpora, and `probe` holds Python readers of a corpus that share no code with the judge.
