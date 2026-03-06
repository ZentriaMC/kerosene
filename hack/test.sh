#!/usr/bin/env bash
# E2E test: boot a FCOS VM, validate with goss, run kerosene playbook.
#
# Env vars:
#   FCOS_HARNESS_SSH_PORT   SSH port forward (default: 2223)
#   FCOS_HARNESS_SSH_KEY    SSH key (set below)
#   KEEP_VM                 Set to 1 to keep VM running after tests
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
export FCOS_HARNESS_WORK_DIR="${root}/tmp/vm"
export FCOS_HARNESS_SSH_KEY="${root}/hack/dev/dev_ed25519"
export FCOS_HARNESS_SSH_PORT="${FCOS_HARNESS_SSH_PORT:-2223}"

chmod 600 "${FCOS_HARNESS_SSH_KEY}"

fh() { fcos-harness "$@"; }

# -- Build Ignition config --
make -C "${root}/hack/init" config.ign
ign="${root}/hack/init/config.ign"

# -- Build kerosene (native, runs on host) --
echo ">>> Building kerosene..."
cargo build --release 2>&1

kerosene_bin="${root}/target/release/kerosene"
if ! [ -f "${kerosene_bin}" ]; then
    echo >&2 ">>> Build failed: kerosene binary not found"
    exit 1
fi

# -- Bring up VM (image + disk + start + wait SSH, with snapshot) --
fh up \
    --ignition "${ign}" \
    --hostname kerosene-test \
    --snapshot ssh-ready \
    --snapshot-goss "${root}/hack/goss.yaml"

trap 'fh down' EXIT

# -- Generate inventory with correct SSH port --
inventory="${FCOS_HARNESS_WORK_DIR}/inventory.yml"
sed "s/ansible_port: .*/ansible_port: ${FCOS_HARNESS_SSH_PORT}/" \
    "${root}/hack/test/inventory.kerosene.yml" > "${inventory}"

# -- Run kerosene E2E test from host --
echo ">>> Running kerosene E2E test playbook..."
cd "${root}"
"${kerosene_bin}" -i "${inventory}" hack/test/playbook.yml

echo ">>> All E2E tests passed!"

# -- Keep VM running if requested --
if [ "${KEEP_VM:-}" = "1" ]; then
    echo ">>> VM is still running (ssh -p ${FCOS_HARNESS_SSH_PORT} core@127.0.0.1)"
    echo ">>> Press Ctrl-C to stop..."
    trap 'fh down; exit 0' INT
    while kill -0 "$(cat "${FCOS_HARNESS_WORK_DIR}/qemu.pid")" 2>/dev/null; do
        sleep 1
    done
fi
