use std::fmt;

/// The rule a refusal names, shared by the ledger's failure leaves and the node's refusal kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefusalClass {
    Inputs,
    Value,
    FeeAndSize,
    Witness,
    Script,
    Datum,
    Metadata,
    Interval,
    Network,
    AccountAndWithdrawal,
    CertificateAndGovernance,
    Collateral,
}

impl RefusalClass {
    /// The class of a ledger failure leaf, by its constructor name.
    pub fn of_ledger_leaf(leaf: &str) -> Option<Self> {
        use RefusalClass as C;
        let class = match leaf {
            "BadInputsUTxO"
            | "SubBadInputsUTxO"
            | "SubInputSetEmptyUTxO"
            | "SubUnspendableUTxONoDatumHash" => C::Inputs,
            "ValueNotConservedUTxO"
            | "ValueNotConservedInLegacyMode"
            | "SubBabbageOutputTooSmallUTxO"
            | "SubOutputTooBigUTxO"
            | "SubOutputBootAddrAttrsTooBig" => C::Value,
            "MaxTxSizeUTxO" | "DijkstraTxRefScriptsSizeTooBig" => C::FeeAndSize,
            "SubMissingVKeyWitnessesUTXOW"
            | "SubInvalidWitnessesUTXOW"
            | "MissingRequiredGuards" => C::Witness,
            "ScriptWitnessNotValidatingUTXOW"
            | "SubScriptWitnessNotValidatingUTXOW"
            | "MissingScriptWitnessesUTXOW"
            | "SubScriptIntegrityHashMismatch"
            | "MissingRedeemers"
            | "SubMissingRedeemers"
            | "ExtraRedeemers"
            | "SubExtraRedeemers"
            | "CollectErrors" => C::Script,
            "SubMissingRequiredDatums"
            | "NotAllowedSupplementalDatums"
            | "SubNotAllowedSupplementalDatums"
            | "MalformedGuardDatums"
            | "SubMalformedGuardDatums" => C::Datum,
            "SubMissingTxMetadata"
            | "SubMissingTxBodyMetadataHash"
            | "SubConflictingMetadataHash" => C::Metadata,
            "SubOutsideValidityIntervalUTxO" => C::Interval,
            "SubWrongNetwork"
            | "SubWrongNetworkInTxBody"
            | "WithdrawalAddressesWithWrongNetwork"
            | "SubWithdrawalAddressesWithWrongNetwork"
            | "DirectDepositAddressesWithWrongNetwork"
            | "SubDirectDepositAddressesWithWrongNetwork"
            | "WrongNetworkInAccountBalanceIntervals"
            | "SubWrongNetworkInAccountBalanceIntervals"
            | "WrongNetworkInStartingAccountBalanceIntervals"
            | "ProposalProcedureNetworkIdMismatch"
            | "TreasuryWithdrawalsNetworkIdMismatch" => C::Network,
            "WithdrawalAccountsMissing"
            | "SubWithdrawalAccountsMissing"
            | "WithdrawalAccountsMissingFromOriginal"
            | "SubWithdrawalAccountsMissingFromOriginal"
            | "WithdrawalAmountsExceedingOriginalBalance"
            | "WithdrawalAmountsInexactInLegacyMode"
            | "BalancesOutsideAccountBalanceIntervals"
            | "SubBalancesOutsideAccountBalanceIntervals"
            | "BalancesOutsideStartingAccountBalanceIntervals"
            | "MissingAccountsInAccountBalanceIntervals"
            | "SubMissingAccountsInAccountBalanceIntervals"
            | "MissingAccountsInStartingAccountBalanceIntervals"
            | "DirectDepositAccountsMissing"
            | "SubDirectDepositAccountsMissing"
            | "StakeKeyNotRegisteredDELEG" => C::AccountAndWithdrawal,
            "RefundIncorrectDELEG"
            | "BlsKeyInvalidProofOfPossession"
            | "VRFKeyHashAlreadyRegistered"
            | "DijkstraTreasuryValueMismatch"
            | "SubTreasuryValueMismatch"
            | "ProposalReturnAccountDoesNotExist"
            | "TreasuryWithdrawalReturnAccountsDoNotExist"
            | "ProposalDepositIncorrect"
            | "ProposalCantFollow"
            | "MalformedProposal"
            | "InvalidPrevGovActionId"
            | "InvalidGuardrailsScriptHash"
            | "GovActionsDoNotExist"
            | "ConflictingCommitteeUpdate"
            | "ExpirationEpochTooSmall"
            | "ZeroTreasuryWithdrawals"
            | "DisallowedVoters"
            | "VotersDoNotExist"
            | "UnelectedCommitteeVoters"
            | "VotingOnExpiredGovAction" => C::CertificateAndGovernance,
            _ => return None,
        };
        Some(class)
    }

    /// The class of a node refusal, by its validation error variant name.
    pub fn of_dolos_kind(kind: &str) -> Option<Self> {
        use RefusalClass as C;
        let class = match kind {
            "InputNotInUTxO"
            | "ReferenceInputNotInUTxO"
            | "TxInsEmpty"
            | "DijkstraInputAlreadySpent" => C::Inputs,
            "PreservationOfValue"
            | "NegativeValue"
            | "MinLovelaceUnreached"
            | "MaxValSizeExceeded" => C::Value,
            "FeeBelowMin"
            | "MaxTxSizeExceeded"
            | "UnknownTxSize"
            | "DijkstraReferenceScriptsTooLarge" => C::FeeAndSize,
            "VKWitnessMissing" | "VKWrongSignature" | "ReqSignerMissing" => C::Witness,
            "ScriptWitnessMissing"
            | "NativeScriptDenial"
            | "ScriptIntegrityHash"
            | "RedeemerMissing"
            | "UnneededRedeemer"
            | "UnneededNativeScript"
            | "DijkstraMalformedScript" => C::Script,
            "DatumMissing" | "UnneededDatum" => C::Datum,
            "MetadataHash" | "DijkstraInvalidMetadata" => C::Metadata,
            "BlockPrecedesValInt" | "BlockExceedsValInt" => C::Interval,
            "TxWrongNetworkID" | "OutputWrongNetworkID" => C::Network,
            "DijkstraInvalidWithdrawal" | "DijkstraCertificateStateUnavailable" => {
                C::AccountAndWithdrawal
            }
            "DijkstraInvalidCertificate" => C::CertificateAndGovernance,
            "CollateralMissing"
            | "CollateralNotInUTxO"
            | "CollateralMinLovelace"
            | "NonLovelaceCollateral"
            | "TooManyCollaterals"
            | "CollateralNotVKeyLocked"
            | "CollateralAnnotation" => C::Collateral,
            _ => return None,
        };
        Some(class)
    }

    /// The name printed in judgement lines.
    pub fn name(self) -> &'static str {
        match self {
            RefusalClass::Inputs => "inputs",
            RefusalClass::Value => "value",
            RefusalClass::FeeAndSize => "fee_and_size",
            RefusalClass::Witness => "witness",
            RefusalClass::Script => "script",
            RefusalClass::Datum => "datum",
            RefusalClass::Metadata => "metadata",
            RefusalClass::Interval => "interval",
            RefusalClass::Network => "network",
            RefusalClass::AccountAndWithdrawal => "account_and_withdrawal",
            RefusalClass::CertificateAndGovernance => "certificate_and_governance",
            RefusalClass::Collateral => "collateral",
        }
    }
}

impl fmt::Display for RefusalClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
