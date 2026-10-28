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

import Cardano.Ledger.Address (Addr (..), Withdrawals (..))
import Cardano.Ledger.Alonzo.Scripts (AsIx (..), PlutusBinary (..), mkBinaryPlutusScript, pattern SpendingPurpose)
import Cardano.Ledger.Alonzo.TxWits (Redeemers (..), TxDats (..))
import Cardano.Ledger.BaseTypes (Network (..), StrictMaybe (..), natVersion)
import Cardano.Ledger.Coin (Coin (..))
import Cardano.Ledger.Credential (Credential (..), StakeReference (..))
import Cardano.Ledger.Dijkstra.Core
import Cardano.Ledger.Mary.Value (AssetName, MaryValue (..), PolicyID (..), multiAssetFromList)
import Cardano.Ledger.Plutus.Data (Data (..), Datum (..), hashData)
import Cardano.Ledger.Plutus.ExUnits (ExUnits (..))
import Cardano.Ledger.Plutus.Language (Language (..))
import Cardano.Ledger.Shelley.Scripts (pattern RequireSignature)
import Cardano.Ledger.Tools (setMinCoinTxOut)
import Cardano.Ledger.TxIn (mkTxInPartial)
import Cardano.Ledger.Val (inject)
import Control.Monad (void)
import Data.ByteString (ByteString)
import qualified Data.ByteString.Base16 as B16
import Data.ByteString.Short (toShort)
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
  -- (program 1.1.0 (lam ctx (force [[[(force (builtin ifThenElse)) [[(builtin equalsInteger) [[(builtin indexByteString) (con bytestring #0a0b)] (con integer 1)]] (con integer 11)]] (delay (con unit ()))] (delay (error))])))
  it "a V3 script calls indexByteString" $
    spendWithV3 "581a01010025333573466e1ccdc72441020a0b004800920161498581"
  -- (program 1.1.0 (lam ctx (force [[[(force (builtin ifThenElse)) [[(builtin equalsByteString) [[[(builtin sliceByteString) (con integer 1)] (con integer 2)] (con bytestring #0a0b0c0d)]] (con bytestring #0b0c)]] (delay (con unit ()))] (delay (error))])))
  it "a V3 script calls sliceByteString" $
    spendWithV3 "582201010025333573466e3cccdc62400490022441040a0b0c0d004881020b0c00149859"
  it "the collateral return of a phase two invalid tx is spent in the same slot" $ do
    addr <- freshKeyAddrNoPtr_
    coll <- sendCoinTo addr (Coin 10_000_000)
    tx <- switchTxToPhase2InvalidLegacyMode (mkBasicTx mkBasicTxBody :: Tx TopTx E)
    let tx' = tx & bodyTxL . collateralInputsTxBodyL .~ [coll] & bodyTxL . collateralReturnTxBodyL .~ SJust (mkBasicTxOut addr (inject (Coin 5_000_000)))
    fixed <- submitPhase2Invalid tx'
    let n = length (fixed ^. bodyTxL . outputsTxBodyL)
        crIn = mkTxInPartial (txIdTx fixed) (fromIntegral n)
    submitTx_ (mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [crIn]) :: Tx TopTx E)

-- | Locks coin at a V3 script given as its CBOR hex, then spends it with that script and no datum.
spendWithV3 :: ByteString -> ImpTestM E ()
spendWithV3 hex = do
  plutus :: PlutusScript E <- expectJust $ mkBinaryPlutusScript PlutusV3 . PlutusBinary . toShort =<< either (const Nothing) Just (B16.decode hex)
  let script = fromPlutusScript plutus
      hash = hashScript script
  locked <- sendCoinTo (Addr Testnet (ScriptHashObj hash) StakeRefNull) (Coin 5_000_000)
  collateral <- makeCollateralInput
  let tx :: Tx TopTx E
      tx =
        mkBasicTx (mkBasicTxBody & inputsTxBodyL .~ [locked] & collateralInputsTxBodyL .~ [collateral] & feeTxBodyL .~ Coin 400_000)
          & witsTxL . scriptTxWitsL .~ [(hash, script)]
          & witsTxL . rdmrsTxWitsL .~ Redeemers [(SpendingPurpose (AsIx 0), (Data (PV1.I 0), ExUnits 1_000_000 1_000_000_000))]
  submitTx_ tx

main :: IO ()
main =
  hspec $
    withImpInit @(LedgerSpec E) $
      modifyImpInitProtVer @E (natVersion @12) $
        modifyImpInitPostSubmitTxHook (dumpTo "/probe/out/supp.jsonl") supplement
