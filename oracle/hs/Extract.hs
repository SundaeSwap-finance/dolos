{-# LANGUAGE DataKinds #-}
{-# LANGUAGE TypeApplications #-}

-- | Writes the Dijkstra Imp spec corpus, one line per LEDGER rule run.
module Main (main) where

import Cardano.Ledger.BaseTypes (natVersion)
import Dump (E, dumpTo)
import qualified Test.Cardano.Ledger.Dijkstra.Imp.BbodySpec as BBODY
import qualified Test.Cardano.Ledger.Dijkstra.Imp.CertSpec as CERT
import qualified Test.Cardano.Ledger.Dijkstra.Imp.EntitiesSpec as ENTITIES
import qualified Test.Cardano.Ledger.Dijkstra.Imp.LedgerSpec as LEDGER
import qualified Test.Cardano.Ledger.Dijkstra.Imp.MempoolSpec as MEMPOOL
import qualified Test.Cardano.Ledger.Dijkstra.Imp.PoolSpec as POOL
import qualified Test.Cardano.Ledger.Dijkstra.Imp.SnapSpec as SNAP
import qualified Test.Cardano.Ledger.Dijkstra.Imp.SubGovSpec as SUBGOV
import qualified Test.Cardano.Ledger.Dijkstra.Imp.SubLedgerSpec as SUBLEDGER
import qualified Test.Cardano.Ledger.Dijkstra.Imp.SubUtxoSpec as SUBUTXO
import qualified Test.Cardano.Ledger.Dijkstra.Imp.SubUtxowSpec as SUBUTXOW
import qualified Test.Cardano.Ledger.Dijkstra.Imp.UtxoSpec as UTXO
import qualified Test.Cardano.Ledger.Dijkstra.Imp.UtxosSpec as UTXOS
import qualified Test.Cardano.Ledger.Dijkstra.Imp.UtxowSpec as UTXOW
import Test.Cardano.Ledger.Dijkstra.ImpTest
import Test.Cardano.Ledger.Imp.Common

main :: IO ()
main =
  hspec $
    withImpInit @(LedgerSpec E) $
      modifyImpInitProtVer @E (natVersion @12) $
        modifyImpInitPostSubmitTxHook (dumpTo "/probe/out/ic.jsonl") $ do
          BBODY.spec
          LEDGER.spec
          MEMPOOL.spec
          SUBLEDGER.spec
          CERT.spec
          ENTITIES.spec
          SUBGOV.spec
          POOL.spec
          SNAP.spec
          UTXOW.spec
          UTXOS.spec
          UTXO.spec
          SUBUTXO.spec
          SUBUTXOW.spec
