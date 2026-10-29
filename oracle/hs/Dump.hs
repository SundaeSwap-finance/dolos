{-# LANGUAGE DataKinds #-}
{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE RankNTypes #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE TypeApplications #-}

-- | One JSON line per LEDGER rule run, written from the Imp post submit hook.
module Dump (E, dumpTo, failurePath) where

import Cardano.Ledger.Alonzo.Plutus.Evaluate (evalTxExUnits)
import Cardano.Ledger.Alonzo.Tx (IsPhase2Valid (..), isPhase2ValidTxL)
import Cardano.Ledger.BaseTypes (Globals (..), Network, epochInfoPure, unNonZero)
import Cardano.Ledger.Binary (serialize')
import Cardano.Ledger.Coin (Coin (..))
import Cardano.Ledger.Core
import Cardano.Ledger.Dijkstra (DijkstraEra)
import Cardano.Ledger.Plutus.ExUnits (ExUnits (..))
import Cardano.Ledger.Shelley.LedgerState (lsCertStateL, lsUTxOStateL)
import Cardano.Ledger.Shelley.Rules (LedgerEnv (..))
import Cardano.Ledger.State (UTxO (..), accountsL, certDStateL, getMinFeeTxUtxo, utxoL)
import Cardano.Slotting.EpochInfo (epochInfoSize)
import Cardano.Slotting.Slot (EpochNo (..), EpochSize (..), SlotNo (..))
import Control.Monad.IO.Class (liftIO)
import Control.State.Transition.Extended (STS (..), TRC (..))
import qualified Data.Aeson as A
import qualified Data.ByteString.Base16 as B16
import qualified Data.ByteString.Char8 as BS8
import qualified Data.ByteString.Lazy.Char8 as BL8
import Data.Char (isAlphaNum, isSpace, isUpper)
import Data.Functor.Identity (runIdentity)
import Data.IORef (IORef, newIORef, readIORef, writeIORef)
import Data.List (dropWhileEnd, isSuffixOf)
import qualified Data.List.NonEmpty as NE
import qualified Data.Map.Strict as Map
import Lens.Micro ((^.))
import System.IO.Unsafe (unsafePerformIO)
import Test.Cardano.Ledger.Dijkstra.ImpTest

type E = DijkstraEra

{-# NOINLINE lastPost #-}
lastPost :: IORef (Maybe (UTxO E))
lastPost = unsafePerformIO (newIORef Nothing)

{-# NOINLINE counter #-}
counter :: IORef Int
counter = unsafePerformIO (newIORef 0)

hex :: BS8.ByteString -> String
hex = BS8.unpack . B16.encode

pairs :: UTxO E -> [[String]]
pairs (UTxO m) =
  [ [hex (serialize' (eraProtVerHigh @E) i), hex (serialize' (eraProtVerHigh @E) o)]
  | (i, o) <- Map.toList m
  ]

-- | The constructor names of one shown failure, from the outer wrapper to the leaf.
failurePath :: String -> [String]
failurePath s = case dropWhile isSpace s of
  c : cs
    | isUpper c ->
        let (rest, after) = span isIdent cs
            name = c : rest
         in name : if "Failure" `isSuffixOf` name then maybe [] failurePath (argument (trim after)) else []
  _ -> []
  where
    isIdent x = isAlphaNum x || x == '_' || x == '\''
    trim = dropWhileEnd isSpace . dropWhile isSpace
    argument a = case soleGroup a of
      Just inner -> Just inner
      Nothing -> case a of
        x : xs | isUpper x && all isIdent xs -> Just a
        _ -> Nothing

-- | The inside of a parenthesised group that spans the whole string.
soleGroup :: String -> Maybe String
soleGroup ('(' : xs) = go (0 :: Int) xs ""
  where
    go 0 [')'] acc = Just (reverse acc)
    go 0 (')' : _) _ = Nothing
    go d (')' : ys) acc = go (d - 1) ys (')' : acc)
    go d ('(' : ys) acc = go (d + 1) ys ('(' : acc)
    go d ('"' : ys) acc = let (lit, rest) = stringLit ys in go d rest (reverse ('"' : lit) ++ acc)
    go d (y : ys) acc = go d ys (y : acc)
    go _ [] _ = Nothing
soleGroup _ = Nothing

-- | A shown string literal after its opening quote, split after its closing quote.
stringLit :: String -> (String, String)
stringLit ('\\' : y : ys) = let (lit, rest) = stringLit ys in ('\\' : y : lit, rest)
stringLit ('"' : ys) = ("\"", ys)
stringLit (y : ys) = let (lit, rest) = stringLit ys in (y : lit, rest)
stringLit [] = ([], [])

globalsJson :: Globals -> EpochNo -> A.Value
globalsJson g e =
  A.object
    [ "network" A..= show (networkId g :: Network)
    , "epoch_size" A..= unEpochSize (runIdentity (epochInfoSize (epochInfoPure g) e))
    , "security_param" A..= unNonZero (securityParameter g)
    ]

dumpTo ::
  FilePath ->
  Globals ->
  TRC (EraRule "LEDGER" E) ->
  Either (NE.NonEmpty (PredicateFailure (EraRule "LEDGER" E))) (State (EraRule "LEDGER" E), [Event (EraRule "LEDGER" E)]) ->
  ImpM t ()
dumpTo out g (TRC (env, st, stAnn)) res = liftIO $ do
  n <- readIORef counter
  writeIORef counter (n + 1)
  prev <- readIORef lastPost
  let tx = stAnn ^. txStAnnTxG
      pre = st ^. lsUTxOStateL . utxoL
      epoch = ledgerEpochNo env
      valid = tx ^. isPhase2ValidTxL == Phase2Valid
      accountsJson s = A.toJSON (s ^. lsCertStateL . certDStateL . accountsL)
      (outcome, post) = case res of
        Left fs ->
          ( A.object
              [ "refused"
                  A..= A.object
                    [ "failures" A..= take 300 (show (NE.toList fs))
                    , "failure_names" A..= map (failurePath . show) (NE.toList fs)
                    ]
              ]
          , Nothing
          )
        Right (st', _) ->
          let p = st' ^. lsUTxOStateL . utxoL
           in ( A.object
                  [ "accepted"
                      A..= A.object ["post_utxo" A..= pairs p, "accounts_post" A..= accountsJson st']
                  ]
              , Just p
              )
      line =
        A.object $
          [ "n" A..= n
          , "slot" A..= unSlotNo (ledgerSlotNo env)
          , "epoch" A..= fmap unEpochNo epoch
          , "follows_prev" A..= (prev == Just pre)
          , "phase2_valid" A..= valid
          , "tx" A..= hex (serialize' (eraProtVerHigh @E) tx)
          , "pparams" A..= A.toJSON (ledgerPp env)
          , "pre_utxo" A..= pairs pre
          , "accounts_pre" A..= accountsJson st
          , "outcome" A..= outcome
          , "ex_units" A..= map evaluationJson (Map.toList (evalTxExUnits (ledgerPp env) tx pre (epochInfo g) (systemStart g)))
          , "min_fee" A..= unCoin (getMinFeeTxUtxo (ledgerPp env) tx pre)
          ]
            ++ [ "globals" A..= globalsJson g e | n == 0, Just e <- [epoch] ]
  writeIORef lastPost (maybe (Just pre) Just post)
  BL8.appendFile out (A.encode line <> "\n")
  where
    evaluationJson (purpose, result) =
      A.object
        [ "purpose" A..= hex (serialize' (eraProtVerHigh @E) purpose)
        , "evaluation" A..= case result of
            Right (ExUnits mem steps) -> A.object ["ran" A..= [mem, steps]]
            Left failure -> A.object ["failed" A..= take 300 (show failure)]
        ]
