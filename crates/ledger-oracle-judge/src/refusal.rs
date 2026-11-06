use std::fmt;

use crate::eval::LeafEval;

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
    Decoding,
}

/// Declares the ledger failure leaves, each with its refusal class and what an evaluation answers under it.
macro_rules! ledger_leaves {
    ($($leaf:ident: $class:ident, $eval:ident;)*) => {
        /// A ledger failure leaf, by its constructor name.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum LedgerLeaf {
            $($leaf,)*
        }

        impl LedgerLeaf {
            /// The leaf a constructor name names, none for a name no leaf has.
            pub fn of_name(name: &str) -> Option<Self> {
                match name {
                    $(stringify!($leaf) => Some(LedgerLeaf::$leaf),)*
                    _ => None,
                }
            }

            /// The constructor name.
            pub fn name(self) -> &'static str {
                match self {
                    $(LedgerLeaf::$leaf => stringify!($leaf),)*
                }
            }

            /// The rule the leaf names.
            pub fn class(self) -> RefusalClass {
                match self {
                    $(LedgerLeaf::$leaf => RefusalClass::$class,)*
                }
            }

            /// What an evaluation of a transaction the ledger refused under the leaf answers.
            pub fn eval(self) -> LeafEval {
                match self {
                    $(LedgerLeaf::$leaf => LeafEval::$eval,)*
                }
            }
        }
    };
}

ledger_leaves! {
    BadInputsUTxO: Inputs, Undecided;
    SubBadInputsUTxO: Inputs, Undecided;
    InputSetEmptyUTxO: Inputs, Undecided;
    SubInputSetEmptyUTxO: Inputs, Undecided;
    UnspendableUTxONoDatumHash: Inputs, Undecided;
    SubUnspendableUTxONoDatumHash: Inputs, Undecided;
    ValueNotConservedUTxO: Value, Undecided;
    ValueNotConservedInLegacyMode: Value, Undecided;
    SubBabbageOutputTooSmallUTxO: Value, Undecided;
    SubOutputTooBigUTxO: Value, Undecided;
    SubOutputBootAddrAttrsTooBig: Value, Undecided;
    BabbageOutputTooSmallUTxO: Value, Undecided;
    OutputTooBigUTxO: Value, Undecided;
    OutputBootAddrAttrsTooBig: Value, Undecided;
    MaxTxSizeUTxO: FeeAndSize, Undecided;
    DijkstraTxRefScriptsSizeTooBig: FeeAndSize, Undecided;
    FeeTooSmallUTxO: FeeAndSize, Evaluate;
    ExUnitsTooBigUTxO: FeeAndSize, Evaluate;
    MissingVKeyWitnessesUTXOW: Witness, Evaluate;
    SubMissingVKeyWitnessesUTXOW: Witness, Evaluate;
    InvalidWitnessesUTXOW: Witness, Evaluate;
    SubInvalidWitnessesUTXOW: Witness, Evaluate;
    MissingRequiredGuards: Witness, Undecided;
    ScriptWitnessNotValidatingUTXOW: Script, Evaluate;
    SubScriptWitnessNotValidatingUTXOW: Script, Evaluate;
    MissingScriptWitnessesUTXOW: Script, Refuse;
    ExtraneousScriptWitnessesUTXOW: Script, Undecided;
    ValidationTagMismatch: Script, Undecided;
    SubScriptIntegrityHashMismatch: Script, Undecided;
    ScriptIntegrityHashMismatch: Script, Undecided;
    MalformedScriptWitnesses: Script, Undecided;
    SubMalformedScriptWitnesses: Script, Undecided;
    MalformedReferenceScripts: Script, Undecided;
    SubMalformedReferenceScripts: Script, Undecided;
    MissingRedeemers: Script, Refuse;
    SubMissingRedeemers: Script, Refuse;
    ExtraRedeemers: Script, Undecided;
    SubExtraRedeemers: Script, Undecided;
    CollectErrors: Script, Refuse;
    MissingRequiredDatums: Datum, Undecided;
    SubMissingRequiredDatums: Datum, Undecided;
    NotAllowedSupplementalDatums: Datum, Undecided;
    SubNotAllowedSupplementalDatums: Datum, Undecided;
    MalformedGuardDatums: Datum, Undecided;
    SubMalformedGuardDatums: Datum, Undecided;
    MissingTxMetadata: Metadata, Undecided;
    SubMissingTxMetadata: Metadata, Undecided;
    MissingTxBodyMetadataHash: Metadata, Undecided;
    SubMissingTxBodyMetadataHash: Metadata, Undecided;
    ConflictingMetadataHash: Metadata, Undecided;
    SubConflictingMetadataHash: Metadata, Undecided;
    OutsideValidityIntervalUTxO: Interval, Undecided;
    SubOutsideValidityIntervalUTxO: Interval, Undecided;
    SubWrongNetwork: Network, Undecided;
    WrongNetwork: Network, Undecided;
    WrongNetworkInTxBody: Network, Undecided;
    SubWrongNetworkInTxBody: Network, Undecided;
    WrongNetworkPOOL: Network, Undecided;
    WithdrawalAddressesWithWrongNetwork: Network, Undecided;
    SubWithdrawalAddressesWithWrongNetwork: Network, Undecided;
    DirectDepositAddressesWithWrongNetwork: Network, Undecided;
    SubDirectDepositAddressesWithWrongNetwork: Network, Undecided;
    WrongNetworkInAccountBalanceIntervals: Network, Undecided;
    SubWrongNetworkInAccountBalanceIntervals: Network, Undecided;
    WrongNetworkInStartingAccountBalanceIntervals: Network, Undecided;
    ProposalProcedureNetworkIdMismatch: Network, Undecided;
    TreasuryWithdrawalsNetworkIdMismatch: Network, Undecided;
    WithdrawalAccountsMissing: AccountAndWithdrawal, Undecided;
    SubWithdrawalAccountsMissing: AccountAndWithdrawal, Undecided;
    WithdrawalAccountsMissingFromOriginal: AccountAndWithdrawal, Undecided;
    SubWithdrawalAccountsMissingFromOriginal: AccountAndWithdrawal, Undecided;
    WithdrawalAmountsExceedingOriginalBalance: AccountAndWithdrawal, Undecided;
    WithdrawalAmountsInexactInLegacyMode: AccountAndWithdrawal, Undecided;
    BalancesOutsideAccountBalanceIntervals: AccountAndWithdrawal, Undecided;
    SubBalancesOutsideAccountBalanceIntervals: AccountAndWithdrawal, Undecided;
    BalancesOutsideStartingAccountBalanceIntervals: AccountAndWithdrawal, Undecided;
    MissingAccountsInAccountBalanceIntervals: AccountAndWithdrawal, Undecided;
    SubMissingAccountsInAccountBalanceIntervals: AccountAndWithdrawal, Undecided;
    MissingAccountsInStartingAccountBalanceIntervals: AccountAndWithdrawal, Undecided;
    DirectDepositAccountsMissing: AccountAndWithdrawal, Undecided;
    SubDirectDepositAccountsMissing: AccountAndWithdrawal, Undecided;
    StakeKeyNotRegisteredDELEG: AccountAndWithdrawal, Undecided;
    RefundIncorrectDELEG: CertificateAndGovernance, Undecided;
    DepositIncorrectDELEG: CertificateAndGovernance, Undecided;
    DelegAccountAlreadyRegistered: CertificateAndGovernance, Undecided;
    StakeKeyHasNonZeroAccountBalanceDELEG: CertificateAndGovernance, Undecided;
    DelegateeDRepNotRegisteredDELEG: CertificateAndGovernance, Undecided;
    DelegateeStakePoolNotRegisteredDELEG: CertificateAndGovernance, Undecided;
    StakePoolCostTooLowPOOL: CertificateAndGovernance, Undecided;
    StakePoolNotRegisteredOnKeyPOOL: CertificateAndGovernance, Undecided;
    StakePoolRetirementWrongEpochPOOL: CertificateAndGovernance, Undecided;
    DijkstraDRepAlreadyRegistered: CertificateAndGovernance, Undecided;
    DijkstraDRepNotRegistered: CertificateAndGovernance, Undecided;
    DijkstraDRepIncorrectDeposit: CertificateAndGovernance, Undecided;
    DijkstraDRepIncorrectRefund: CertificateAndGovernance, Undecided;
    DijkstraCommitteeIsUnknown: CertificateAndGovernance, Undecided;
    DijkstraCommitteeHasPreviouslyResigned: CertificateAndGovernance, Undecided;
    BlsKeyInvalidProofOfPossession: CertificateAndGovernance, Undecided;
    VRFKeyHashAlreadyRegistered: CertificateAndGovernance, Undecided;
    DijkstraTreasuryValueMismatch: CertificateAndGovernance, Undecided;
    SubTreasuryValueMismatch: CertificateAndGovernance, Undecided;
    ProposalReturnAccountDoesNotExist: CertificateAndGovernance, Undecided;
    TreasuryWithdrawalReturnAccountsDoNotExist: CertificateAndGovernance, Undecided;
    ProposalDepositIncorrect: CertificateAndGovernance, Undecided;
    ProposalCantFollow: CertificateAndGovernance, Undecided;
    MalformedProposal: CertificateAndGovernance, Undecided;
    InvalidPrevGovActionId: CertificateAndGovernance, Undecided;
    InvalidGuardrailsScriptHash: CertificateAndGovernance, Undecided;
    GovActionsDoNotExist: CertificateAndGovernance, Undecided;
    ConflictingCommitteeUpdate: CertificateAndGovernance, Undecided;
    ExpirationEpochTooSmall: CertificateAndGovernance, Undecided;
    ZeroTreasuryWithdrawals: CertificateAndGovernance, Undecided;
    DisallowedVoters: CertificateAndGovernance, Undecided;
    VotersDoNotExist: CertificateAndGovernance, Undecided;
    UnelectedCommitteeVoters: CertificateAndGovernance, Undecided;
    VotingOnExpiredGovAction: CertificateAndGovernance, Undecided;
    InsufficientCollateral: Collateral, Undecided;
    ScriptsNotPaidUTxO: Collateral, Undecided;
    CollateralContainsNonADA: Collateral, Undecided;
    TooManyCollateralInputs: Collateral, Undecided;
    NoCollateralInputs: Collateral, Undecided;
    IncorrectTotalCollateralField: Collateral, Undecided;
    PtrPresentInCollateralReturn: Collateral, Undecided;
    DecoderErrorDeserialiseFailure: Decoding, Undecided;
    DecoderErrorCustom: Decoding, Undecided;
    DecoderErrorLeftover: Decoding, Undecided;
}

impl fmt::Display for LedgerLeaf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl RefusalClass {
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
            RefusalClass::Decoding => "decoding",
        }
    }
}

impl fmt::Display for RefusalClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The class of a node refusal by its error variant, unknown for a variant the class table does not name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DolosClass {
    Rule(RefusalClass),
    NoRule,
    Unknown,
}

impl DolosClass {
    /// The name printed in judgement lines.
    pub fn name(self) -> &'static str {
        match self {
            DolosClass::Rule(class) => class.name(),
            DolosClass::NoRule => "none",
            DolosClass::Unknown => "unknown",
        }
    }
}
