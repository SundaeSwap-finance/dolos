//! The matching of a transaction against a tx predicate, for every RPC
//! version.

/// Emits the matching of a transaction against a tx predicate for the schema
/// module `u5c`, reading pattern fields through `criterion` and transaction
/// bodies through `members`.
macro_rules! matchers {
    ($u5c:ident, criterion = $criterion:path, members = $members:path) => {
        fn matches_address(
            pattern: &$u5c::cardano::AddressPattern,
            address: &::bytes::Bytes,
        ) -> bool {
            use ::pallas::ledger::addresses::Address;

            let exact_matches =
                $criterion(&pattern.exact_address).is_none_or(|exact| address == exact);

            let delegation_matches =
                $criterion(&pattern.delegation_part).is_none_or(|delegation| {
                    match Address::from_bytes(address).unwrap() {
                        Address::Shelley(s) => s.delegation().to_vec().eq(delegation),
                        _ => false,
                    }
                });
            let payment_matches = $criterion(&pattern.payment_part).is_none_or(|payment| {
                match Address::from_bytes(address).unwrap() {
                    Address::Shelley(s) => s.payment().to_vec().eq(payment),
                    _ => false,
                }
            });

            exact_matches && delegation_matches && payment_matches
        }

        fn outputs_match_address<'a>(
            pattern: &$u5c::cardano::AddressPattern,
            outputs: impl IntoIterator<Item = &'a $u5c::cardano::TxOutput>,
        ) -> bool {
            outputs
                .into_iter()
                .any(|o| matches_address(pattern, &o.address))
        }

        fn outputs_match_asset<'a>(
            asset_pattern: &$u5c::cardano::AssetPattern,
            outputs: impl IntoIterator<Item = &'a $u5c::cardano::TxOutput>,
        ) -> bool {
            outputs
                .into_iter()
                .any(|o| matches_asset(asset_pattern, &o.assets))
        }

        fn matches_asset<'a>(
            asset_pattern: &$u5c::cardano::AssetPattern,
            assets: impl IntoIterator<Item = &'a $u5c::cardano::Multiasset>,
        ) -> bool {
            assets.into_iter().any(|ma| {
                if $criterion(&asset_pattern.policy_id)
                    .is_some_and(|policy| policy.ne(&ma.policy_id))
                {
                    return false;
                }
                let Some(name) = $criterion(&asset_pattern.asset_name) else {
                    return true;
                };
                ma.assets.iter().any(|ma| name.eq(&ma.name))
            })
        }

        fn matches_output(
            pattern: &$u5c::cardano::TxOutputPattern,
            output: &$u5c::cardano::TxOutput,
        ) -> bool {
            let $u5c::cardano::TxOutputPattern { address, asset } = pattern;

            let address_match = address
                .as_ref()
                .is_none_or(|addr_pattern| matches_address(addr_pattern, &output.address));

            let asset_match = asset
                .as_ref()
                .is_none_or(|asset_pattern| matches_asset(asset_pattern, &output.assets));

            address_match && asset_match
        }

        fn credential_hash_eq(cred: &$u5c::cardano::StakeCredential, hash: &[u8]) -> bool {
            use $u5c::cardano::stake_credential::StakeCredential as SC;
            match &cred.stake_credential {
                Some(SC::AddrKeyHash(h) | SC::ScriptHash(h)) => h == hash,
                None => false,
            }
        }

        fn drep_hash_eq(drep: &$u5c::cardano::DRep, hash: &[u8]) -> bool {
            use $u5c::cardano::d_rep::Drep;
            match &drep.drep {
                Some(Drep::AddrKeyHash(h) | Drep::ScriptHash(h)) => h == hash,
                _ => false,
            }
        }

        fn matches_stake_credential_pattern(
            pattern: &$u5c::cardano::StakeCredential,
            actual: &$u5c::cardano::StakeCredential,
        ) -> bool {
            pattern.stake_credential.is_none()
                || pattern.stake_credential == actual.stake_credential
        }

        fn cert_involves_stake_credential(
            cert: &$u5c::cardano::certificate::Certificate,
            hash: &[u8],
        ) -> bool {
            use $u5c::cardano::certificate::Certificate as C;
            match cert {
                C::StakeRegistration(c) | C::StakeDeregistration(c) => credential_hash_eq(c, hash),
                C::StakeDelegation(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::RegCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::UnregCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::VoteDelegCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::StakeVoteDelegCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::StakeRegDelegCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::VoteRegDelegCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::StakeVoteRegDelegCert(c) => c
                    .stake_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::AuthCommitteeHotCert(c) => {
                    c.committee_cold_credential
                        .as_ref()
                        .is_some_and(|sc| credential_hash_eq(sc, hash))
                        || c.committee_hot_credential
                            .as_ref()
                            .is_some_and(|sc| credential_hash_eq(sc, hash))
                }
                C::ResignCommitteeColdCert(c) => c
                    .committee_cold_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::RegDrepCert(c) => c
                    .drep_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::UnregDrepCert(c) => c
                    .drep_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::UpdateDrepCert(c) => c
                    .drep_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::MirCert(c) => c.to.iter().any(|t| {
                    t.stake_credential
                        .as_ref()
                        .is_some_and(|sc| credential_hash_eq(sc, hash))
                }),
                _ => false,
            }
        }

        fn cert_involves_pool(
            cert: &$u5c::cardano::certificate::Certificate,
            hash: &[u8],
        ) -> bool {
            use $u5c::cardano::certificate::Certificate as C;
            match cert {
                C::StakeDelegation(c) => c.pool_keyhash == hash,
                C::PoolRegistration(c) => c.operator == hash,
                C::PoolRetirement(c) => c.pool_keyhash == hash,
                C::StakeVoteDelegCert(c) => c.pool_keyhash == hash,
                C::StakeRegDelegCert(c) => c.pool_keyhash == hash,
                C::StakeVoteRegDelegCert(c) => c.pool_keyhash == hash,
                _ => false,
            }
        }

        fn cert_involves_drep(
            cert: &$u5c::cardano::certificate::Certificate,
            hash: &[u8],
        ) -> bool {
            use $u5c::cardano::certificate::Certificate as C;
            match cert {
                C::VoteDelegCert(c) => c.drep.as_ref().is_some_and(|d| drep_hash_eq(d, hash)),
                C::StakeVoteDelegCert(c) => {
                    c.drep.as_ref().is_some_and(|d| drep_hash_eq(d, hash))
                }
                C::VoteRegDelegCert(c) => c.drep.as_ref().is_some_and(|d| drep_hash_eq(d, hash)),
                C::StakeVoteRegDelegCert(c) => {
                    c.drep.as_ref().is_some_and(|d| drep_hash_eq(d, hash))
                }
                C::RegDrepCert(c) => c
                    .drep_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::UnregDrepCert(c) => c
                    .drep_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                C::UpdateDrepCert(c) => c
                    .drep_credential
                    .as_ref()
                    .is_some_and(|sc| credential_hash_eq(sc, hash)),
                _ => false,
            }
        }

        fn matches_certificate_pattern<'a>(
            pattern: &$u5c::cardano::CertificatePattern,
            certs: impl IntoIterator<Item = &'a $u5c::cardano::Certificate>,
        ) -> bool {
            use $u5c::cardano::certificate::Certificate as Cert;
            use $u5c::cardano::certificate_pattern::CertificateType;

            let Some(ref cert_type) = pattern.certificate_type else {
                return true;
            };

            certs.into_iter().any(|cert| {
                let Some(ref c) = cert.certificate else {
                    return false;
                };

                match cert_type {
                    CertificateType::StakeRegistration(pat) => {
                        matches!(c, Cert::StakeRegistration(cred) if matches_stake_credential_pattern(pat, cred))
                    }
                    CertificateType::StakeDeregistration(pat) => {
                        matches!(c, Cert::StakeDeregistration(cred) if matches_stake_credential_pattern(pat, cred))
                    }
                    CertificateType::StakeDelegation(pat) => {
                        if let Cert::StakeDelegation(deleg) = c {
                            let cred_match = pat.stake_credential.as_ref().is_none_or(|p| {
                                deleg
                                    .stake_credential
                                    .as_ref()
                                    .is_some_and(|a| matches_stake_credential_pattern(p, a))
                            });
                            let pool_match = pat.pool_keyhash.is_empty()
                                || pat.pool_keyhash == deleg.pool_keyhash;
                            cred_match && pool_match
                        } else {
                            false
                        }
                    }
                    CertificateType::PoolRegistration(pat) => {
                        if let Cert::PoolRegistration(reg) = c {
                            let operator_match =
                                pat.operator.is_empty() || pat.operator == reg.operator;
                            let pool_match =
                                pat.pool_keyhash.is_empty() || pat.pool_keyhash == reg.operator;
                            operator_match && pool_match
                        } else {
                            false
                        }
                    }
                    CertificateType::PoolRetirement(pat) => {
                        if let Cert::PoolRetirement(ret) = c {
                            let pool_match = pat.pool_keyhash.is_empty()
                                || pat.pool_keyhash == ret.pool_keyhash;
                            let epoch_match = pat.epoch == 0 || pat.epoch == ret.epoch;
                            pool_match && epoch_match
                        } else {
                            false
                        }
                    }
                    CertificateType::AnyStakeCredential(hash) => {
                        cert_involves_stake_credential(c, hash)
                    }
                    CertificateType::AnyPoolKeyhash(hash) => cert_involves_pool(c, hash),
                    CertificateType::AnyDrep(hash) => cert_involves_drep(c, hash),
                }
            })
        }

        fn matches_cardano_pattern(
            tx_pattern: &$u5c::cardano::TxPattern,
            tx: &$u5c::cardano::Tx,
        ) -> bool {
            let members: Vec<_> = $members(tx).collect();
            let inputs = || {
                members
                    .iter()
                    .flat_map(|x| x.inputs.iter())
                    .filter_map(|x| x.as_output.as_ref())
            };
            let outputs = || members.iter().flat_map(|x| x.outputs.iter());
            let referenced = || {
                members
                    .iter()
                    .flat_map(|x| x.reference_inputs.iter())
                    .filter_map(|x| x.as_output.as_ref())
            };
            let collateral_inputs = || {
                tx.collateral
                    .iter()
                    .flat_map(|x| x.collateral.iter())
                    .filter_map(|x| x.as_output.as_ref())
            };
            let collateral_return = || {
                tx.collateral
                    .iter()
                    .filter_map(|x| x.collateral_return.as_ref())
            };
            let moved = || {
                inputs()
                    .chain(collateral_inputs())
                    .chain(collateral_return())
                    .chain(outputs())
            };

            let has_address_match = tx_pattern.has_address.as_ref().is_none_or(|addr_pattern| {
                outputs_match_address(addr_pattern, moved().chain(referenced()))
            });

            let consumes_match = tx_pattern
                .consumes
                .as_ref()
                .is_none_or(|out_pattern| inputs().any(|x| matches_output(out_pattern, x)));

            let mints_asset_match = tx_pattern.mints_asset.as_ref().is_none_or(|asset_pattern| {
                matches_asset(asset_pattern, members.iter().flat_map(|x| x.mint.iter()))
            });

            let moves_asset_match = tx_pattern
                .moves_asset
                .as_ref()
                .is_none_or(|asset_pattern| outputs_match_asset(asset_pattern, moved()));

            let produces_match = tx_pattern
                .produces
                .as_ref()
                .is_none_or(|out_pattern| outputs().any(|x| matches_output(out_pattern, x)));

            let has_certificate_match =
                tx_pattern
                    .has_certificate
                    .as_ref()
                    .is_none_or(|cert_pattern| {
                        matches_certificate_pattern(
                            cert_pattern,
                            members.iter().flat_map(|x| x.certificates.iter()),
                        )
                    });

            has_address_match
                && consumes_match
                && mints_asset_match
                && moves_asset_match
                && produces_match
                && has_certificate_match
        }

        fn matches_chain(
            chain: &$u5c::watch::any_chain_tx_pattern::Chain,
            tx: &$u5c::cardano::Tx,
        ) -> bool {
            match chain {
                $u5c::watch::any_chain_tx_pattern::Chain::Cardano(tx_pattern) => {
                    matches_cardano_pattern(tx_pattern, tx)
                }
            }
        }

        impl<'a> $crate::serve::grpc::predicate::Predicate for &'a $u5c::watch::TxPredicate {
            type Pattern = &'a $u5c::watch::AnyChainTxPattern;

            fn into_clauses(self) -> $crate::serve::grpc::predicate::Clauses<Self> {
                let $u5c::watch::TxPredicate {
                    r#match,
                    not,
                    all_of,
                    any_of,
                } = self;
                $crate::serve::grpc::predicate::Clauses {
                    matching: r#match.as_ref(),
                    not: not.iter().collect(),
                    all_of: all_of.iter().collect(),
                    any_of: any_of.iter().collect(),
                }
            }
        }

        pub(super) fn apply_predicate(
            predicate: &$u5c::watch::TxPredicate,
            tx: &$u5c::cardano::Tx,
        ) -> bool {
            let Ok(admitted) = $crate::serve::grpc::predicate::evaluate(predicate, &mut |pattern| {
                Ok::<_, std::convert::Infallible>(
                    pattern
                        .chain
                        .as_ref()
                        .is_none_or(|chain| matches_chain(chain, tx)),
                )
            });
            admitted
        }
    };
}

pub(super) use matchers;
