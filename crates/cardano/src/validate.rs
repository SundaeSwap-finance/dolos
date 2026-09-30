use crate::model::FixedNamespace;
use std::borrow::Cow;

use dolos_core::{
    ChainError, ChainPoint, Domain, EraCbor, Genesis, MempoolAwareUtxoStore, MempoolStore,
    MempoolTx, StateStore,
};

use pallas::ledger::{
    primitives::{NetworkId, TransactionInput},
    traverse::{Era, MultiEraInput, MultiEraOutput, MultiEraTx},
};
use pallas_validate::utils::{
    CertState, DijkstraPlutusParams, DijkstraProtParams, DijkstraRegistrationState,
    MultiEraProtocolParameters,
};
use tracing::debug;

pub fn validate_tx<D: Domain>(
    cbor: &[u8],
    utxos: &MempoolAwareUtxoStore<D>,
    tip: Option<ChainPoint>,
    genesis: &Genesis,
) -> Result<MempoolTx, ChainError> {
    let active = crate::load_effective_pparams::<D>(utxos.state())?;
    let era = submission_era(active.ensure_protocol_version()?)?;
    let tx = decode_submission(era, cbor)?;
    let hash = tx.hash();
    let pparams = validation_parameters(&active, genesis)?;

    let network_id = match genesis.shelley.network_id.as_ref() {
        Some(network) => match network.as_str() {
            "Mainnet" => Some(NetworkId::Mainnet.into()),
            "Testnet" => Some(NetworkId::Testnet.into()),
            _ => None,
        },
        None => None,
    }
    .ok_or_else(|| ChainError::GenesisFieldMissing("network id".into()))?;

    let env = pallas_validate::utils::Environment {
        prot_params: pparams,
        prot_magic: genesis
            .shelley
            .network_magic
            .ok_or_else(|| ChainError::GenesisFieldMissing("network magic".into()))?,
        block_slot: tip.ok_or(ChainError::NoActiveEpoch)?.slot(),
        network_id,
        acnt: if era == Era::Dijkstra {
            None
        } else {
            Some(pallas_validate::utils::AccountState::default())
        },
    };

    let input_refs = tx.requires().iter().map(From::from).collect();

    let utxos_matches = utxos.get_utxos(input_refs)?;

    let mut pallas_utxos = pallas_validate::utils::UTxOs::new();

    for (txoref, eracbor) in utxos_matches.iter() {
        let tx_in = TransactionInput {
            transaction_id: txoref.0,
            index: txoref.1.into(),
        };

        let input = MultiEraInput::AlonzoCompatible(<Box<Cow<'_, TransactionInput>>>::from(
            Cow::Owned(tx_in),
        ));

        let eracbor = eracbor.as_ref();

        let output = MultiEraOutput::try_from(eracbor)?;

        pallas_utxos.insert(input, output);
    }

    let mut cert_state = certificate_state(&tx, utxos)?;
    pallas_validate::phase1::validate_tx(&tx, 0, &env, &pallas_utxos, &mut cert_state)?;

    let report = evaluate_decoded::<D>(&tx, utxos, &env.prot_params)?;

    for eval in report.iter() {
        if !eval.success {
            return Err(ChainError::Phase2ValidationRejected(eval.logs.clone()));
        }
    }

    debug!(
        phase1 = true,
        phase2 = true,
        redeemer_count = report.len(),
        "tx validated"
    );

    let era = u16::from(tx.era());
    let payload = EraCbor(era, cbor.into());

    let tx = MempoolTx::new(hash, payload, report);

    Ok(tx)
}

pub fn evaluate_tx<D: Domain>(
    cbor: &[u8],
    utxos: &MempoolAwareUtxoStore<D>,
    genesis: &Genesis,
) -> Result<pallas_validate::phase2::EvalReport, ChainError> {
    let active = crate::load_effective_pparams::<D>(utxos.state())?;
    let tx = decode_submission(submission_era(active.ensure_protocol_version()?)?, cbor)?;
    let pparams = validation_parameters(&active, genesis)?;
    evaluate_decoded::<D>(&tx, utxos, &pparams)
}

fn evaluate_decoded<D: Domain>(
    tx: &MultiEraTx<'_>,
    utxos: &MempoolAwareUtxoStore<D>,
    pparams: &MultiEraProtocolParameters,
) -> Result<pallas_validate::phase2::EvalReport, ChainError> {
    use dolos_core::TxoRef;

    let eras = crate::eras::load_era_summary::<D>(utxos.state())?;

    // `eras` keeps slot_length/timestamp in seconds; the Plutus ScriptContext
    // needs POSIXTime in milliseconds. The conversion lives in the helper.
    let slot_config = eras.to_pallas_slot_config();

    let input_refs = tx.requires().iter().map(From::from).collect();

    let utxos: pallas_validate::utils::UtxoMap = utxos
        .get_utxos(input_refs)?
        .into_iter()
        .map(|(TxoRef(a, b), eracbor)| {
            let era = eracbor.era().try_into()?;

            Ok::<_, ChainError>((
                pallas_validate::utils::TxoRef::from((a, b)),
                pallas_validate::utils::EraCbor::from((era, eracbor.cbor().into())),
            ))
        })
        .collect::<Result<_, _>>()?;

    let report = pallas_validate::phase2::evaluate_tx(tx, pparams, &utxos, &slot_config)
        .map_err(|e| ChainError::Phase2EvaluationError(e.to_string()))?;

    Ok(report)
}

/// Submission has no era tag in its signed envelope. Only known protocol
/// versions select an era; unknown future versions must not inherit this
/// subset.
pub fn submission_era(protocol: (u64, u64)) -> Result<Era, ChainError> {
    match protocol {
        (0..=1, _) => Ok(Era::Byron),
        (2, _) => Ok(Era::Shelley),
        (3, _) => Ok(Era::Allegra),
        (4, _) => Ok(Era::Mary),
        (5..=6, _) => Ok(Era::Alonzo),
        (7..=8, _) => Ok(Era::Babbage),
        (9..=11, _) => Ok(Era::Conway),
        (12, 0) => Ok(Era::Dijkstra),
        _ => Err(ChainError::InvalidConfig(format!(
            "unsupported submission protocol {}.{}",
            protocol.0, protocol.1
        ))),
    }
}

/// Native validation consumes the actual active parameters and Dijkstra
/// genesis. This does not extend the supported ledger/evaluator subset in
/// Pallas.
pub fn validation_parameters(
    pp: &crate::model::PParamsSet,
    genesis: &Genesis,
) -> Result<MultiEraProtocolParameters, ChainError> {
    let protocol = pp.ensure_protocol_version()?;
    if submission_era(protocol)? != Era::Dijkstra {
        // Preserve the existing earlier-era parameter projection.
        return Ok(crate::utils::pparams_to_pallas(pp));
    }
    let missing = |name: &str| ChainError::PParamsNotFound(name.into());
    let u32_param = |x: u64, name: &str| {
        u32::try_from(x).map_err(|_| ChainError::InvalidConfig(format!("{name} exceeds u32")))
    };
    let g = genesis
        .dijkstra
        .as_ref()
        .ok_or_else(|| ChainError::GenesisFieldMissing("dijkstra".into()))?;
    let multiplier = num_rational::Ratio::<i64>::approximate_float(g.ref_script_cost_multiplier)
        .filter(|x| *x.numer() > 0 && *x.denom() > 0)
        .ok_or_else(|| {
            ChainError::InvalidConfig("invalid reference-script cost multiplier".into())
        })?;
    let plutus = pp
        .cost_models_plutus_v3()
        .map(|cost_model_v3| {
            Ok::<_, ChainError>(DijkstraPlutusParams {
                cost_model_v3,
                execution_costs: pp
                    .execution_costs()
                    .ok_or_else(|| missing("execution costs"))?,
                max_tx_ex_units: pp
                    .max_tx_ex_units()
                    .ok_or_else(|| missing("max tx ex units"))?,
                collateral_percentage: pp
                    .collateral_percentage()
                    .ok_or_else(|| missing("collateral percentage"))?,
                max_collateral_inputs: pp
                    .max_collateral_inputs()
                    .ok_or_else(|| missing("max collateral inputs"))?,
                minfee_refscript_cost_per_byte: pp
                    .min_fee_ref_script_cost_per_byte()
                    .ok_or_else(|| missing("reference script cost"))?,
                max_ref_script_size_per_tx: u32_param(
                    g.max_ref_script_size_per_tx,
                    "max reference script size",
                )?,
                ref_script_cost_stride: u32_param(
                    g.ref_script_cost_stride,
                    "reference script cost stride",
                )?,
                ref_script_cost_multiplier: pallas::ledger::primitives::RationalNumber {
                    numerator: *multiplier.numer() as u64,
                    denominator: *multiplier.denom() as u64,
                },
            })
        })
        .transpose()?;
    Ok(MultiEraProtocolParameters::Dijkstra(DijkstraProtParams {
        protocol_version: protocol,
        system_start: chrono::DateTime::from_timestamp(pp.ensure_system_start()? as i64, 0)
            .ok_or_else(|| ChainError::InvalidConfig("invalid system start".into()))?
            .fixed_offset(),
        epoch_length: pp.ensure_epoch_length()?,
        slot_length: pp.ensure_slot_length()?,
        minfee_a: u32_param(
            pp.min_fee_a().ok_or_else(|| missing("min fee a"))?,
            "min fee a",
        )?,
        minfee_b: u32_param(
            pp.min_fee_b().ok_or_else(|| missing("min fee b"))?,
            "min fee b",
        )?,
        max_transaction_size: u32_param(
            pp.max_transaction_size()
                .ok_or_else(|| missing("max transaction size"))?,
            "max transaction size",
        )?,
        ada_per_utxo_byte: pp
            .ada_per_utxo_byte()
            .ok_or_else(|| missing("ada per utxo byte"))?,
        max_value_size: pp
            .max_value_size()
            .ok_or_else(|| missing("max value size"))?,
        key_deposit: pp.key_deposit(),
        plutus,
    }))
}

fn certificate_state<D: Domain>(
    tx: &MultiEraTx<'_>,
    utxos: &MempoolAwareUtxoStore<D>,
) -> Result<CertState, ChainError> {
    use crate::model::AccountState;
    use pallas::ledger::primitives::dijkstra::Certificate;
    let mut state = CertState::default();
    if let Some(native) = tx.as_dijkstra() {
        for cert in native.transaction_body.certificates.iter().flatten() {
            if let Certificate::Reg(credential, _) = cert {
                // The synced account namespace is authoritative, including
                // absence. Only the transaction-local state is
                // changed by phase one.
                let key = pallas::codec::minicbor::to_vec(credential).unwrap();
                let account = utxos
                    .state()
                    .read_entity_typed::<AccountState>(AccountState::NS, &key.into())?;
                let registered = account.is_some_and(|a| a.is_registered());
                state.dijkstra_registrations.insert(
                    credential.clone(),
                    if registered {
                        DijkstraRegistrationState::Registered
                    } else {
                        DijkstraRegistrationState::Unregistered
                    },
                );
            }
        }
        // Pending accepted registrations also reserve their credentials. Do not
        // let two otherwise independent transactions register the same account.
        let mut pending = utxos.mempool().peek_pending();
        pending.extend(utxos.mempool().peek_inflight());
        for entry in pending {
            let pending_tx = MultiEraTx::try_from(&entry.payload)?;
            for cert in pending_tx.certs() {
                if let Some(credential) = crate::pallas_extras::cert_as_stake_registration(&cert) {
                    if let Some(value) = state.dijkstra_registrations.get_mut(&credential) {
                        *value = DijkstraRegistrationState::Registered;
                    }
                }
            }
        }
    }
    Ok(state)
}

fn decode_submission(era: Era, cbor: &[u8]) -> Result<MultiEraTx<'_>, ChainError> {
    let mut decoder = pallas::codec::minicbor::Decoder::new(cbor);
    decoder.skip()?;
    if decoder.position() != cbor.len() {
        return Err(
            pallas::codec::minicbor::decode::Error::message("trailing transaction bytes").into(),
        );
    }
    if era == Era::Dijkstra {
        // The traversal decoder also accepts block-only envelopes. A submitted
        // payload must satisfy the mempool rule, since these exact bytes relay.
        let mut wire = pallas::codec::minicbor::Decoder::new(cbor);
        let indefinite = wire.datatype()? == pallas::codec::minicbor::data::Type::ArrayIndef;
        let _: pallas::ledger::primitives::dijkstra::MempoolTransaction = wire.decode()?;
        if indefinite && wire.datatype()? == pallas::codec::minicbor::data::Type::Break {
            wire.skip()?;
        }
        if wire.position() != cbor.len() {
            return Err(pallas::codec::minicbor::decode::Error::message(
                "unexpected mempool transaction fields",
            )
            .into());
        }
    }
    Ok(MultiEraTx::decode_for_era(era, cbor)?)
}
