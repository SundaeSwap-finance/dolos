{-# LANGUAGE DataKinds #-}
{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE NumericUnderscores #-}
{-# LANGUAGE OverloadedLists #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE PatternSynonyms #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE TypeApplications #-}

-- | Writes the supplement corpus, scenarios whose shapes the Imp spec corpus lacks.
module Main (main) where

import Cardano.Ledger.Address (Withdrawals (..))
import Cardano.Ledger.Alonzo.TxWits (TxDats (..))
import Cardano.Ledger.BaseTypes (StrictMaybe (..), natVersion)
import Cardano.Ledger.Coin (Coin (..))
import Cardano.Ledger.Credential (Credential (..))
import Cardano.Ledger.Dijkstra.Core
import Cardano.Ledger.Mary.Value (AssetName, MaryValue (..), PolicyID (..), multiAssetFromList)
import Cardano.Ledger.Plutus.Data (Data (..), Datum (..), hashData)
import Cardano.Ledger.Shelley.Scripts (pattern RequireSignature)
import Cardano.Ledger.Tools (setMinCoinTxOut)
import Cardano.Ledger.TxIn (mkTxInPartial)
import Cardano.Ledger.Val (inject)
import Control.Monad (void)
import Data.Maybe (listToMaybe)
import qualified Data.OMap.Strict as OMap
import Dump (E, dumpTo)
import Lens.Micro ((&), (.~), (^.))
import qualified PlutusLedgerApi.V1 as PV1
import Test.Cardano.Ledger.Core.Utils (txInAt)
import Test.Cardano.Ledger.Dijkstra.ImpTest
import Test.Cardano.Ledger.Imp.Common

supplement :: SpecWith (ImpInit (LedgerSpec E))
supplement = describe "SUPPLEMENT" $ do
  it "a tx spends a child output of the batch before it in one slot" $ do
    pp <- getsPParams id
    spent <- freshFundedTxIn
    addr <- freshKeyAddr_
    let sub :: Tx SubTx E
        sub = mkBasicTx $ mkBasicTxBody & inputsTxBodyL .~ [spent] & outputsTxBodyL .~ [setMinCoinTxOut pp $ mkBasicTxOut addr mempty]
        y = mkTxInPartial (txIdTx sub) 0
    submitTx_ (mkTopTxWithSubTxs [sub])
    submitTx_ (mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [y]) :: Tx TopTx E)
  it "a tx spends an input a child of the batch before it spent in one slot" $ do
    x <- freshFundedTxIn
    submitTx_ (mkTopTxWithSubTxs [mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [x]) :: Tx SubTx E])
    void $ trySubmitTx (mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [x]) :: Tx TopTx E)
  it "the collateral return of a phase two invalid tx is spent in the next slot" $ do
    addr <- freshKeyAddrNoPtr_
    coll <- sendCoinTo addr (Coin 10_000_000)
    tx <- switchTxToPhase2InvalidLegacyMode (mkBasicTx mkBasicTxBody :: Tx TopTx E)
    let tx' = tx & bodyTxL . collateralInputsTxBodyL .~ [coll] & bodyTxL . collateralReturnTxBodyL .~ SJust (mkBasicTxOut addr (inject (Coin 5_000_000)))
    fixed <- submitPhase2Invalid tx'
    let n = length (fixed ^. bodyTxL . outputsTxBodyL)
        crIn = mkTxInPartial (txIdTx fixed) (fromIntegral n)
    passTick
    submitTx_ (mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [crIn]) :: Tx TopTx E)
  it "a child spends a token output" $ do
    pp <- getsPParams id
    policy <- PolicyID <$> (impAddNativeScript . RequireSignature =<< freshKeyHash)
    name <- arbitrary @AssetName
    holder <- freshKeyAddrNoPtr_
    let token = multiAssetFromList [(policy, name, 1)]
    minted <- submitTx (mkBasicTx (mkBasicTxBody & mintTxBodyL .~ token & outputsTxBodyL .~ [setMinCoinTxOut pp (mkBasicTxOut holder (MaryValue mempty token))]) :: Tx TopTx E)
    passTick
    funded <- freshFundedTxIn
    receiver <- freshKeyAddrNoPtr_
    let sub :: Tx SubTx E
        sub = mkBasicTx $ mkBasicTxBody & inputsTxBodyL .~ [txInAt 0 minted, funded] & outputsTxBodyL .~ [setMinCoinTxOut pp (mkBasicTxOut receiver (MaryValue mempty token))]
    submitTx_ (mkTopTxWithSubTxs [sub])
  it "a child output holds a hash datum given in the child witnesses, and a later child spends it" $ do
    pp <- getsPParams id
    holder <- freshKeyAddrNoPtr_
    funded <- freshFundedTxIn
    let datum = Data (PV1.I 42) :: Data E
        maker :: Tx SubTx E
        maker =
          mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [funded] & outputsTxBodyL .~ [setMinCoinTxOut pp (mkBasicTxOut holder mempty & datumTxOutL .~ DatumHash (hashData datum))])
            & witsTxL . datsTxWitsL .~ TxDats [(hashData datum, datum)]
    top <- submitTx (mkTopTxWithSubTxs [maker])
    made <- expectJust . listToMaybe . OMap.elems $ top ^. bodyTxL . subTransactionsTxBodyL
    passTick
    funded' <- freshFundedTxIn
    receiver <- freshKeyAddrNoPtr_
    let spender :: Tx SubTx E
        spender = mkBasicTx $ mkBasicTxBody & inputsTxBodyL .~ [txInAt 0 made, funded'] & outputsTxBodyL .~ [setMinCoinTxOut pp (mkBasicTxOut receiver mempty)]
    submitTx_ (mkTopTxWithSubTxs [spender])
  it "a child output carries a reference script" $ do
    pp <- getsPParams id
    holder <- freshKeyAddrNoPtr_
    funded <- freshFundedTxIn
    script <- fromNativeScript . RequireSignature <$> freshKeyHash
    let sub :: Tx SubTx E
        sub = mkBasicTx $ mkBasicTxBody & inputsTxBodyL .~ [funded] & outputsTxBodyL .~ [setMinCoinTxOut pp (mkBasicTxOut holder mempty & referenceScriptTxOutL .~ SJust script)]
    submitTx_ (mkTopTxWithSubTxs [sub])
  it "a tx names one input as both a spent and a reference input" $ do
    x <- freshFundedTxIn
    void $ trySubmitTx (mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [x] & referenceInputsTxBodyL .~ [x]) :: Tx TopTx E)
  it "a partial withdrawal, then a legacy drain of the remainder in one slot" $ do
    account <- registerStakeCredential . KeyHashObj =<< freshKeyHash
    submitTx_ (mkBasicTx (mkBasicTxBody & directDepositsTxBodyL .~ DirectDeposits [(account, Coin 1_000_000)]) :: Tx TopTx E)
    passTick
    submitTx_ (mkBasicTx (mkBasicTxBody & withdrawalsTxBodyL .~ Withdrawals [(account, Coin 400_000)]) :: Tx TopTx E)
    drain <- switchTxToLegacyMode (mkBasicTx (mkBasicTxBody & withdrawalsTxBodyL .~ Withdrawals [(account, Coin 600_000)]))
    void $ trySubmitTx drain
  it "a registration, then a withdrawal from that account in one slot" $ do
    account <- registerStakeCredential . KeyHashObj =<< freshKeyHash
    submitTx_ (mkBasicTx (mkBasicTxBody & withdrawalsTxBodyL .~ Withdrawals [(account, Coin 0)]) :: Tx TopTx E)

main :: IO ()
main =
  hspec $
    withImpInit @(LedgerSpec E) $
      modifyImpInitProtVer @E (natVersion @12) $
        modifyImpInitPostSubmitTxHook (dumpTo "/probe/out/supp.jsonl") supplement
