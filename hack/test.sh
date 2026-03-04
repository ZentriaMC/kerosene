#!/usr/bin/env bash
# E2E test: boot a FCOS VM, validate with goss, run kerosene playbook.
# Uses QEMU savevm/loadvm to cache a booted VM snapshot for fast restarts.
#
# Env vars:
#   TEST_SSH_PORT      SSH port forward (default: 2223)
#   REBUILD_SNAPSHOT   Set to 1 to force snapshot recreation
#   KEEP_VM            Set to 1 to keep VM running after tests
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
work_dir="${root}/tmp/vm"
ssh_port="${TEST_SSH_PORT:-2223}"
ssh_key="${root}/hack/dev/dev_ed25519"

snapshot_disk="${work_dir}/fcos-snapshot.qcow2"
snapshot_name="ssh-ready"
snapshot_hash_file="${work_dir}/snapshot.hash"
monitor_sock="${work_dir}/qemu-monitor.sock"
pid_file="${work_dir}/qemu.pid"

chmod 600 "${ssh_key}"

fh() {
    fcos-harness --work-dir "${work_dir}" "$@"
}
fh_ssh() {
    fh ssh --ssh-key "${ssh_key}" --ssh-port "${ssh_port}" "$@"
}

# ---------------------------------------------------------------------------
# Build Ignition config
# ---------------------------------------------------------------------------
make -C "${root}/hack/init" config.ign
ign="${root}/hack/init/config.ign"

# ---------------------------------------------------------------------------
# Build kerosene (native, runs on host)
# ---------------------------------------------------------------------------
echo ">>> Building kerosene..."
cargo build --release 2>&1

kerosene_bin="${root}/target/release/kerosene"
if ! [ -f "${kerosene_bin}" ]; then
    echo >&2 ">>> Build failed: kerosene binary not found"
    exit 1
fi

# ---------------------------------------------------------------------------
# Ensure FCOS base image
# ---------------------------------------------------------------------------
fh image

# ---------------------------------------------------------------------------
# Check if a valid VM snapshot exists
# ---------------------------------------------------------------------------
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

current_hash="$(sha256 "${ign}")"
use_snapshot=false

if [ "${REBUILD_SNAPSHOT:-}" != "1" ] \
    && [ -f "${snapshot_disk}" ] \
    && [ -f "${snapshot_hash_file}" ] \
    && [ "$(cat "${snapshot_hash_file}")" = "${current_hash}" ] \
    && qemu-img snapshot -l "${snapshot_disk}" 2>/dev/null | grep -q "${snapshot_name}"; then
    use_snapshot=true
    echo ">>> Valid VM snapshot found, skipping boot+goss"
fi

# ---------------------------------------------------------------------------
# Create snapshot if needed: boot fresh, wait for SSH, goss, savevm, quit
# ---------------------------------------------------------------------------
if [ "${use_snapshot}" = false ]; then
    echo ">>> Creating VM snapshot (first run or config changed)..."
    rm -f "${snapshot_disk}" "${snapshot_hash_file}"
    fh disk --base "${work_dir}/fcos.qcow2" --overlay "${snapshot_disk}"

    fh start \
        --disk "${snapshot_disk}" \
        --ignition "${ign}" \
        --ssh-port "${ssh_port}" \
        --hostname kerosene-test \
        --serial-log "${work_dir}/serial-test.log" \
        --qmp "${monitor_sock}" \
        --pid-file "${pid_file}"

    cleanup_snapshot() {
        fh stop --pid-file "${pid_file}" 2>/dev/null || true
        rm -f "${monitor_sock}"
    }
    trap cleanup_snapshot EXIT

    echo ">>> Waiting for SSH..."
    fh_ssh --wait 180 -- true

    echo ">>> Running goss validation..."
    fh goss "${root}/hack/goss.yaml" --ssh-key "${ssh_key}" --ssh-port "${ssh_port}" --retry-timeout-secs 30

    echo ">>> Saving VM snapshot '${snapshot_name}'..."
    fh qmp --socket "${monitor_sock}" savevm "${snapshot_name}"

    echo ">>> Stopping snapshot VM..."
    fh qmp --socket "${monitor_sock}" quit
    sleep 1
    fh stop --pid-file "${pid_file}" 2>/dev/null || true
    rm -f "${monitor_sock}"
    trap - EXIT

    echo "${current_hash}" > "${snapshot_hash_file}"
    echo ">>> Snapshot created"
fi

# ---------------------------------------------------------------------------
# Boot VM from snapshot (instant restore, ephemeral writes)
# ---------------------------------------------------------------------------
echo ">>> Booting test VM from snapshot..."
fh start \
    --disk "${snapshot_disk}" \
    --ignition "${ign}" \
    --ssh-port "${ssh_port}" \
    --hostname kerosene-test \
    --serial-log "${work_dir}/serial-test.log" \
    --loadvm "${snapshot_name}" \
    --pid-file "${pid_file}"

cleanup() {
    echo ">>> Shutting down test VM..."
    fh stop --pid-file "${pid_file}" 2>/dev/null || true
}
trap cleanup EXIT

echo ">>> Waiting for SSH (should be instant from snapshot)..."
fh_ssh --wait 30 -- true

# ---------------------------------------------------------------------------
# Run kerosene E2E test from host
# ---------------------------------------------------------------------------
echo ">>> Running kerosene E2E test playbook..."
cd "${root}"
"${kerosene_bin}" -i hack/test/inventory.kerosene.yml hack/test/playbook.yml

echo ">>> All E2E tests passed!"

# ---------------------------------------------------------------------------
# Keep VM running if requested
# ---------------------------------------------------------------------------
if [ "${KEEP_VM:-}" = "1" ]; then
    echo ">>> VM is still running (ssh -p ${ssh_port} core@127.0.0.1)"
    echo ">>> Press Ctrl-C to stop..."
    trap cleanup INT
    wait "$(cat "${pid_file}")"
fi
