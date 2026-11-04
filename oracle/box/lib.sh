# Functions the oracle box scripts share, reading ROOT, SCRATCH, REV, MIN_FREE_G, HERE, TARGET and IN_IMAGE.

PALLAS=${PALLAS:-scratch/pallas}
IN_IMAGE=${IN_IMAGE:-$HERE/in-image.sh}
# One line for each unit of the target dir, the tree ids of the scratch trees its last build read and its output files.
BUILT="$ROOT/${TARGET:-target}/built-units"
# The arms that plant a defect, each judged on the records plants/<arm>.records names.
PLANTED_ARMS="colret coin inflight rodrigo-reverted rob-reverted cost revalidate revalidate-drops subouts byron-attrs byron-signer byron-guard chaincode minfee-subtx emptyguards tag24 v1v2 byron-out tag24-ref unreg-refund drep-deposit upper-inclusive slot-off-by-one horizon-removed system-start cost-models guard-ref-reverted native-ref-reencoded v2-ref-dropped v4-ref-refused v1-ref-exposed pointer-refused script-payment-swap signatories-unsorted withdrawal-order cert-unwrapped script-guard-admitted v3-cert-wrapped v2-withdrawal-unwrapped top-guards-admitted"

# The arms whose switch an overlay diff adds, one per line.
switches() {
  sed -n 's/^+.*oracle_plant("\([a-z0-9-]*\)").*/\1/p' "$@" | sort -u
}

# Whether an arm is a switch of the overlay, judged on the one overlay build with ORACLE_PLANT naming it.
overlay_arm() {
  switches "$HERE"/overlay/*.diff | grep -q -x -F "$1"
}

# Whether an arm plants its defect in pallas.
pallas_arm() {
  if overlay_arm "$1"; then
    switches "$HERE/overlay/pallas.diff" | grep -q -x -F "$1"
  else
    sed -n 's#^+++ b/##p' "$HERE/plants/$1.diff" 2>/dev/null | grep -q '^pallas'
  fi
}

# The cargo arguments of the crate test an arm runs in place of the judge, none for an arm the judge runs.
crate_test() {
  case $1 in
    cost-models) echo "-p dolos-cardano --features strict --test musashi_cost_models" ;;
    *) return 1 ;;
  esac
}

# The Dolos files that name the pallas commit Dolos builds against.
PALLAS_PIN_FILES="Cargo.toml Cargo.lock"

# The pallas entries of the file $3 at the commit $2 of the repository $1, its pallas lines for Cargo.toml and its pallas packages for Cargo.lock.
pallas_entries() {
  git -C "$1" show "$2:$3" 2>/dev/null | case $3 in
    Cargo.lock) awk -v RS= '/name = "pallas/' ;;
    *) grep '^pallas' ;;
  esac
}

# The files under the paths after $3 that differ between the commits $2 and $3 of the repository $1, a pallas pin file only when its pallas entries differ.
differing_files() {
  repo=$1
  from=$2
  to=$3
  shift 3
  names=$(git -C "$repo" diff --name-only "$from" "$to" -- "$@") || return
  echo "$names" | while read -r name; do
    case " $PALLAS_PIN_FILES " in
      *" $name "*)
        if [ "$(pallas_entries "$repo" "$from" "$name")" = "$(pallas_entries "$repo" "$to" "$name")" ]; then
          continue
        fi
        ;;
    esac
    if [ -n "$name" ]; then
      echo "$name"
    fi
  done
}

# The Dolos files whose change puts an arm in the quick tier, the pin files for a pallas arm.
plant_files() {
  if pallas_arm "$1"; then
    printf '%s\n' $PALLAS_PIN_FILES
    return
  fi
  if overlay_arm "$1"; then
    awk -v call="oracle_plant(\"$1\")" '/^\+\+\+ b\// { file = substr($0, 7) } /^\+/ && index($0, call) { print file }' \
      "$HERE/overlay/dolos.diff" | sort -u
    return
  fi
  sed -n 's#^+++ b/##p' "$HERE/plants/$1.diff"
}

# Whether two test logs are the same but for the test harness's run time.
same_log() {
  sed 's/; finished in [0-9.]*s$//' "$1" >"$1.untimed"
  sed 's/; finished in [0-9.]*s$//' "$2" >"$2.untimed"
  cmp -s "$1.untimed" "$2.untimed"
}

# Exits when the build root has less than MIN_FREE_G gigabytes free.
require_free() {
  free=$(df -BG --output=avail "$ROOT" | tail -1 | tr -dc 0-9)
  if [ "$free" -lt "$MIN_FREE_G" ]; then
    echo "REFUSED free=${free}G below ${MIN_FREE_G}G at $ROOT"
    exit 2
  fi
}

# Puts the scratch tree at REV of the bare repository $ROOT/dolos.git with no planted change.
reset_scratch() {
  if [ ! -d "$ROOT/$SCRATCH/.git" ]; then
    git clone -q --no-checkout "$ROOT/dolos.git" "$ROOT/$SCRATCH"
  fi
  git -C "$ROOT/$SCRATCH" fetch -q "$ROOT/dolos.git" "$REV"
  git -C "$ROOT/$SCRATCH" checkout -q -f --detach FETCH_HEAD
  git -C "$ROOT/$SCRATCH" clean -q -fdx
}

# The tree id of a work tree's files, staged in an index of its own, or none when the tree is absent.
tree_of() {
  if [ ! -d "$1/.git" ]; then
    echo none
    return
  fi
  index="$1/.git/built-index"
  rm -f "$index"
  GIT_INDEX_FILE=$index git -C "$1" add -A
  GIT_INDEX_FILE=$index git -C "$1" write-tree
  rm -f "$index"
}

# The files that differ between an old and a new tree id of a work tree, each with a prefix, every file of the new tree when the old one is not in the repository.
changed_files() {
  if [ "$3" = none ]; then
    return
  fi
  if [ "$2" != none ] && git -C "$1" cat-file -e "$2^{tree}" 2>/dev/null; then
    git -C "$1" diff --name-only "$2" "$3"
  else
    git -C "$1" ls-tree -r --name-only "$3"
  fi | sed "s#^#$4#"
}

# The first dep-info file rustc wrote beside one of the output files of a unit, its outputs given comma separated.
dep_info() {
  echo "$1" | tr , '\n' | while read -r out; do
    out=${out#/w/}
    stem=$(basename "$out" | sed -E 's/^lib(.*)\.(rlib|rmeta|so|a)$/\1/')
    # A build script's output is a link named build-script-<name> in a dir named after the package and the unit hash.
    case $stem in
      build-script-*)
        dir=$(dirname "$out")
        stem="$(echo "$stem" | tr - _)-${dir##*-}"
        ;;
    esac
    if [ -f "$ROOT/$(dirname "$out")/$stem.d" ]; then
      echo "$ROOT/$(dirname "$out")/$stem.d"
      break
    fi
  done
}

# The output files of each unit a message file names, comma separated, one line per unit, only the fresh ones when $2 is fresh.
units() {
  only=
  if [ "${2:-}" = fresh ]; then
    only='"fresh":true'
  fi
  grep '"reason":"compiler-artifact"' "$1" | grep -F -e "$only" |
    sed -n 's/.*"filenames":\[\([^]]*\)\].*/\1/p' | tr -d '"'
}

# One line for each unit cargo reports fresh whose dep-info names a file changed between the trees of its last build and the trees $2 and $3, or that has no dep-info.
fresh_reading() {
  units "$1" fresh | while read -r outs; do
    built=$(awk -v outs="$outs" '$3 == outs { print $1, $2 }' "$BUILT" 2>/dev/null || true)
    built=${built:-none none}
    changed="$1.changed-$(echo "$built" | tr ' ' -)"
    if [ ! -f "$changed" ]; then
      {
        changed_files "$ROOT/$SCRATCH" "${built% *}" "$2" ""
        changed_files "$ROOT/$PALLAS" "${built#* }" "$3" "/w/$PALLAS/"
      } >"$changed"
    fi
    d=$(dep_info "$outs")
    if [ -z "$d" ]; then
      echo "STALE unit=$outs dep_info=missing"
      continue
    fi
    tr ' ' '\n' <"$d" | sed "s/:\$//; s#^/w/$SCRATCH/##" | grep -x -F -f "$changed" | sort -u |
      sed "s#^#STALE unit=$outs file=#"
  done
}

# Records the trees $2 and $3 for each unit a message file names, keeping the record of every other unit.
record_units() {
  units "$1" >"$1.units"
  {
    if [ -f "$BUILT" ]; then
      awk 'NR == FNR { named[$0] = 1; next } !($3 in named)' "$1.units" "$BUILT"
    fi
    sed "s#^#$2 $3 #" "$1.units"
  } >"$BUILT.new"
  mv "$BUILT.new" "$BUILT"
}

# Runs cargo with the arguments in the scratch tree, cargo's messages in $1, and refuses when cargo reused a unit that reads a file changed since that unit's last build.
build_checked() {
  json=$1
  shift
  dolos=$(tree_of "$ROOT/$SCRATCH")
  pallas=$(tree_of "$ROOT/$PALLAS")
  if [ ! -f "$BUILT" ]; then
    # With no record of any build, every file gets a new time, so cargo rebuilds each unit that reads one.
    for tree in "$SCRATCH" "$PALLAS"; do
      if [ -d "$ROOT/$tree/.git" ]; then
        git -C "$ROOT/$tree" ls-files -z --cached --others --exclude-standard | (cd "$ROOT/$tree" && xargs -0 touch)
        # The index takes the new times too, so a later checkout of the same commit keeps each file and its time.
        git -C "$ROOT/$tree" update-index -q --refresh || true
      fi
    done
  fi
  # A build that fails or reuses a stale unit leaves units built from more than one tree, so it removes the record and the next build starts cold.
  if ! sh "$IN_IMAGE" "$SCRATCH" cargo "$@" --message-format=json >"$json" 2>"$json.err"; then
    rm -f "$BUILT"
    echo "BUILD_FAILED log=$json.err"
    return 1
  fi
  rm -f "$json".changed-*
  fresh_reading "$json" "$dolos" "$pallas" >"$json.stale"
  if [ -s "$json.stale" ]; then
    rm -f "$BUILT"
    head -4 "$json.stale"
    return 1
  fi
  # A fresh unit that reads no changed file has the inputs it would have at the current trees.
  record_units "$json" "$dolos" "$pallas"
}

# Builds the test binary of the scratch tree's dolos library, cargo's messages in $1 and the rest of the arguments passed to cargo.
build_judge() {
  json=$1
  shift
  build_checked "$json" test --release -p dolos --lib --no-run "$@"
}

# The executable cargo built that a message file names, the test binary of a no run test build or the binary of a build.
executable() {
  sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' "$1" | tail -1
}
