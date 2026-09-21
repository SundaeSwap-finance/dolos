"""Compare the protocol parameters the follower serves against the node's.

One line per parameter, every line ending IDENTICAL, DIFFERS or REPORTED, and a
non-zero exit if anything differs. A parameter the node answers that the wire
format has no field for is REPORTED by name, because that is a gap in the format
rather than a disagreement between the two.

The wire format encodes an unset scalar and a zero the same way, so a parameter
the reply leaves out is read as the zero it means and the line says so. Nothing
is treated as agreeing because it was absent.

Usage: compare-params.py

Reads the node over its socket with cardano-cli and the follower over UtxoRPC
with grpcurl. Read only.
"""

import json
import os
import subprocess
import sys

CARDANO_CLI = os.environ.get("CARDANO_CLI", "/usr/local/bin/cardano-cli")
GRPCURL = os.environ.get("GRPCURL", "/opt/build/grpcurl")
DOLOS_GRPC = os.environ.get("DOLOS_GRPC", "localhost:50051")
READ_PARAMS = "utxorpc.v1alpha.query.QueryService/ReadParams"

# Parameters the node answers for this chain that the wire format names no field
# for. Serving one needs a field in the format and a value in the parameter set,
# so neither side can be at fault for its absence and it is reported, not
# compared.
NAMED_GAPS = [
    "leiosAnnouncementPeriodLength",
    "leiosCommitteeSize",
    "leiosDiffusionPeriodLength",
    "leiosQuorumStakeThreshold",
    "leiosVotePeriodLength",
    "maxEndorserBlockExecutionUnits",
    "maxEndorserBlockReferencesSize",
    "maxEndorserBlockTxsSize",
    "maxPledgeLeverage",
    "maxRefScriptSizePerBlock",
    "maxRefScriptSizePerEndorserBlock",
    "maxRefScriptSizePerTx",
    "minPoolMargin",
    "refScriptCostMultiplier",
    "refScriptCostStride",
]

# Follower field, node field, and how the follower encodes the value.
SCALARS = [
    ("maxTxSize", "maxTxSize", "plain"),
    ("maxBlockBodySize", "maxBlockBodySize", "plain"),
    ("maxBlockHeaderSize", "maxBlockHeaderSize", "plain"),
    ("poolRetirementEpochBound", "poolRetireMaxEpoch", "plain"),
    ("desiredNumberOfPools", "stakePoolTargetNum", "plain"),
    ("maxValueSize", "maxValueSize", "plain"),
    ("collateralPercentage", "collateralPercentage", "plain"),
    ("maxCollateralInputs", "maxCollateralInputs", "plain"),
    ("minCommitteeSize", "committeeMinSize", "plain"),
    ("committeeTermLimit", "committeeMaxTermLength", "plain"),
    ("governanceActionValidityPeriod", "govActionLifetime", "plain"),
    ("drepInactivityPeriod", "dRepActivity", "plain"),
    ("coinsPerUtxoByte", "utxoCostPerByte", "big"),
    ("minFeeCoefficient", "txFeePerByte", "big"),
    ("minFeeConstant", "txFeeFixed", "big"),
    ("stakeKeyDeposit", "stakeAddressDeposit", "big"),
    ("poolDeposit", "stakePoolDeposit", "big"),
    ("minPoolCost", "minPoolCost", "big"),
    ("governanceActionDeposit", "govActionDeposit", "big"),
    ("drepDeposit", "dRepDeposit", "big"),
]

RATIOS = [
    ("poolInfluence", "poolPledgeInfluence"),
    ("monetaryExpansion", "monetaryExpansion"),
    ("treasuryExpansion", "treasuryCut"),
    ("minFeeScriptRefCostPerByte", "minFeeRefScriptCostPerByte"),
]

EX_UNITS = [
    ("maxExecutionUnitsPerTransaction", "maxTxExecutionUnits"),
    ("maxExecutionUnitsPerBlock", "maxBlockExecutionUnits"),
]

POOL_THRESHOLDS = [
    "motionNoConfidence",
    "committeeNormal",
    "committeeNoConfidence",
    "hardForkInitiation",
    "ppSecurityGroup",
]

DREP_THRESHOLDS = [
    "motionNoConfidence",
    "committeeNormal",
    "committeeNoConfidence",
    "updateToConstitution",
    "hardForkInitiation",
    "ppNetworkGroup",
    "ppEconomicGroup",
    "ppTechnicalGroup",
    "ppGovGroup",
    "treasuryWithdrawal",
]

COST_MODELS = [
    ("plutusV1", "PlutusV1"),
    ("plutusV2", "PlutusV2"),
    ("plutusV3", "PlutusV3"),
    ("plutusV4", "PlutusV4"),
]

TOLERANCE = 1e-9


def run(argv):
    done = subprocess.run(argv, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{argv[0]} failed with {done.returncode}: {done.stderr.strip()}")
    return json.loads(done.stdout)


def node_params():
    return run([CARDANO_CLI, "query", "protocol-parameters"])


def follower_params():
    reply = run(
        [GRPCURL, "-plaintext", "-d", "{}", DOLOS_GRPC, READ_PARAMS]
    )
    served = reply.get("values", {}).get("cardano")
    if served is None:
        sys.exit("the follower answered no cardano parameters")
    return served


class Line:
    """One comparison, carrying both readings and how each was read."""

    def __init__(self, name, node, follower, verdict):
        self.name = name
        self.node = node
        self.follower = follower
        self.verdict = verdict

    def __str__(self):
        return f"{self.name:34} node {self.node:28} follower {self.follower:28} {self.verdict}"


def agreed(name, node, follower, same):
    return Line(name, node, follower, "IDENTICAL" if same else "DIFFERS")


def read_plain(served, field):
    raw = served.get(field)
    if raw is None:
        return 0, "absent, so 0"
    return int(raw), str(int(raw))


def read_big(served, field):
    raw = served.get(field)
    if raw is None:
        return 0, "absent, so 0"
    if "int" not in raw:
        return None, "outside the int64 range"
    return int(raw["int"]), str(int(raw["int"]))


def read_ratio(served, field):
    raw = served.get(field)
    if raw is None:
        return None, "absent"
    num = int(raw.get("numerator", 0))
    den = int(raw.get("denominator", 0))
    if den == 0:
        return None, f"{num} over zero"
    return num / den, f"{num}/{den}"


def ratio_of(value):
    return float(value)


def compare_scalars(node, served, lines):
    for field, node_field, kind in SCALARS:
        want = int(node[node_field])
        got, shown = read_plain(served, field) if kind == "plain" else read_big(served, field)
        lines.append(agreed(field, str(want), shown, got == want))


def compare_ratios(node, served, lines):
    for field, node_field in RATIOS:
        want = ratio_of(node[node_field])
        got, shown = read_ratio(served, field)
        same = got is not None and abs(got - want) < TOLERANCE
        lines.append(agreed(field, repr(want), shown, same))


def compare_ex_units(node, served, lines):
    for field, node_field in EX_UNITS:
        want = node[node_field]
        raw = served.get(field)
        if raw is None:
            lines.append(agreed(field, "set", "absent", False))
            continue
        got = (int(raw.get("memory", 0)), int(raw.get("steps", 0)))
        want = (int(want["memory"]), int(want["steps"]))
        lines.append(
            agreed(field, f"{want[0]},{want[1]}", f"{got[0]},{got[1]}", got == want)
        )


def compare_thresholds(node, served, lines, field, node_field, order):
    raw = served.get(field)
    if raw is None:
        lines.append(agreed(field, f"{len(order)} values", "absent", False))
        return

    got = [
        int(x.get("numerator", 0)) / int(x.get("denominator", 1))
        for x in raw.get("thresholds", [])
    ]
    want = [ratio_of(node[node_field][key]) for key in order]
    same = len(got) == len(want) and all(
        abs(a - b) < TOLERANCE for a, b in zip(got, want)
    )
    lines.append(agreed(field, f"{len(want)} values", f"{len(got)} values", same))


def compare_cost_models(node, served, lines):
    models = served.get("costModels") or {}
    for field, node_field in COST_MODELS:
        want = [int(x) for x in node.get("costModels", {}).get(node_field, [])]
        raw = models.get(field)
        if raw is None:
            shown = "absent"
            got = None
        else:
            got = [int(x) for x in raw.get("values", [])]
            shown = f"{len(got)} entries"
        lines.append(
            agreed(
                f"costModels.{field}",
                f"{len(want)} entries",
                shown,
                got == want,
            )
        )


def compare_protocol_version(node, served, lines):
    raw = served.get("protocolVersion") or {}
    got = (int(raw.get("major", 0)), int(raw.get("minor", 0)))
    want = (
        int(node["protocolVersion"]["major"]),
        int(node["protocolVersion"]["minor"]),
    )
    lines.append(
        agreed(
            "protocolVersion",
            f"{want[0]}.{want[1]}",
            f"{got[0]}.{got[1]}",
            got == want,
        )
    )


def main():
    node = node_params()
    served = follower_params()

    lines = []
    compare_scalars(node, served, lines)
    compare_ratios(node, served, lines)
    compare_ex_units(node, served, lines)
    compare_protocol_version(node, served, lines)
    compare_thresholds(
        node, served, lines, "poolVotingThresholds", "poolVotingThresholds", POOL_THRESHOLDS
    )
    compare_thresholds(
        node, served, lines, "drepVotingThresholds", "dRepVotingThresholds", DREP_THRESHOLDS
    )
    compare_cost_models(node, served, lines)

    for line in lines:
        print(line)

    for name in NAMED_GAPS:
        value = json.dumps(node.get(name))
        print(f"{name:34} node {value:28} follower {'no field':28} REPORTED")

    differing = [line.name for line in lines if line.verdict == "DIFFERS"]
    print(
        f"PARAMS {len(lines)} compared, {len(differing)} differing, "
        f"{len(NAMED_GAPS)} reported as gaps"
    )

    if differing:
        print("DIFFERS " + " ".join(differing))
        return 1

    print("IDENTICAL")
    return 0


if __name__ == "__main__":
    sys.exit(main())
