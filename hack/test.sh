#!/usr/bin/env bash
# E2E test: boot a FCOS VM, validate with goss, run kerosene playbook.
# Uses QEMU savevm/loadvm to cache a booted VM snapshot for fast restarts.
#
# Env vars:
#   QEMU_EFI_FW       (required) Path to UEFI firmware
#   TEST_SSH_PORT      SSH port forward (default: 2223)
#   REBUILD_SNAPSHOT   Set to 1 to force snapshot recreation
#   KEEP_VM            Set to 1 to keep VM running after tests
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
tmp_vm="${root}/tmp/vm"
ssh_port="${TEST_SSH_PORT:-2223}"
serial_log="${tmp_vm}/serial-test.log"
goss_version="v0.4.9"

# Snapshot paths
snapshot_disk="${tmp_vm}/fcos-snapshot.qcow2"
snapshot_name="ssh-ready"
snapshot_hash_file="${tmp_vm}/snapshot.hash"
monitor_sock="${tmp_vm}/qemu-monitor.sock"

chmod 600 "${root}/hack/dev/dev_ed25519"

ssh_common_opts=(
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
    -o ConnectTimeout=5
    -i "${root}/hack/dev/dev_ed25519"
)
ssh_opts=("${ssh_common_opts[@]}" -p "${ssh_port}")
scp_opts=("${ssh_common_opts[@]}" -P "${ssh_port}")
remote() { ssh "${ssh_opts[@]}" core@127.0.0.1 "$@"; }

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Send a command over the QEMU Machine Protocol (QMP) unix socket.
qmp() {
    local sock="$1" cmd="$2"
    python3 -c "
import socket, json, sys

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])
sock.settimeout(120)

# QMP greeting
sock.recv(65536)
# Negotiate capabilities
sock.sendall(b'{\"execute\":\"qmp_capabilities\"}\n')
sock.recv(65536)
# Send command
sock.sendall((sys.argv[2] + '\n').encode())
try:
    resp = json.loads(sock.recv(65536))
    if 'error' in resp:
        print('QMP error: ' + json.dumps(resp['error']), file=sys.stderr)
        sys.exit(1)
    ret = resp.get('return', '')
    if ret:
        print(ret, file=sys.stderr)
except Exception:
    pass  # expected when sending quit
sock.close()
" "${sock}" "${cmd}"
}

wait_for_ssh() {
    local timeout="$1"
    local elapsed=0
    while (( elapsed < timeout )); do
        if ssh "${ssh_opts[@]}" core@127.0.0.1 true 2>/dev/null; then
            echo "${elapsed}"
            return 0
        fi
        sleep 5
        elapsed=$(( elapsed + 5 ))
    done
    return 1
}

# ---------------------------------------------------------------------------
# Detect arch and QEMU binary
# ---------------------------------------------------------------------------
: "${QEMU_EFI_FW:?QEMU_EFI_FW must be set}"

qemu_args=(
    -nodefaults
    -no-user-config
    -display none

    -cpu host
    -smp 4
    -m 4G
    -serial "file:${serial_log}"

    -rtc base=utc
    -netdev "user,id=user.0,hostfwd=tcp::${ssh_port}-:22,hostname=kerosene-test"
    -device virtio-rng-pci
    -device virtio-scsi-pci,id=scsi0,num_queues=4
    -device virtio-serial-pci
    -device virtio-net-pci,netdev=user.0
    -device qemu-xhci -usb
    -drive "if=none,id=root-disk0,file=${snapshot_disk},format=qcow2"
    -device scsi-hd,bus=scsi0.0,drive=root-disk0
)

case "$(uname -s).$(uname -m)" in
    Linux.x86_64)
        arch="x86_64"
        goss_arch="amd64"
        qemu=qemu-system-x86_64
        qemu_args+=(-machine q35,accel=kvm -drive "if=pflash,file=${QEMU_EFI_FW},format=raw,unit=0,readonly=on")
        ;;
    Darwin.arm64)
        arch="aarch64"
        goss_arch="arm64"
        qemu=qemu-system-aarch64
        qemu_args+=(-machine virt,accel=hvf -bios "${QEMU_EFI_FW}")
        ;;
    Linux.aarch64)
        arch="aarch64"
        goss_arch="arm64"
        qemu=qemu-system-aarch64
        qemu_args+=(-machine virt,accel=kvm -bios "${QEMU_EFI_FW}")
        ;;
    *)
        echo >&2 "Unsupported platform"
        exit 1
        ;;
esac

# ---------------------------------------------------------------------------
# Build Ignition config
# ---------------------------------------------------------------------------
make -C "${root}/hack/init" config.ign

ign="${root}/hack/init/config.ign"
qemu_args+=(-fw_cfg "name=opt/com.coreos/config,file=${ign}")

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
# Prepare disk image
# ---------------------------------------------------------------------------
mkdir -p "${tmp_vm}"
basedisk="${tmp_vm}/fcos.qcow2"

if ! [ -f "${basedisk}" ]; then
    echo ">>> Downloading FCOS image..."
    qemu_artifact="$(curl -s "https://builds.coreos.fedoraproject.org/streams/next.json" \
        | jq -ecr --arg arch "${arch}" '.architectures[$arch].artifacts.qemu')"
    url="$(jq -ecr '.formats["qcow2.xz"].disk.location' <<< "${qemu_artifact}")"
    sha256_expected="$(jq -ecr '.formats["qcow2.xz"].disk.sha256' <<< "${qemu_artifact}")"

    compressed="${tmp_vm}/fcos.qcow2.xz"
    curl -L -f -o "${compressed}" "${url}"

    if ! (sha256sum "${compressed}" 2>/dev/null || shasum -a 256 "${compressed}") | grep -q -F "${sha256_expected}"; then
        echo >&2 ">>> Checksum mismatch"
        exit 1
    fi
    xz -d < "${compressed}" > "${basedisk}"
    rm -f "${compressed}"
fi

# ---------------------------------------------------------------------------
# Fetch goss binary for the VM
# ---------------------------------------------------------------------------
goss_bin="${tmp_vm}/goss-linux-${goss_arch}"
if ! [ -f "${goss_bin}" ]; then
    echo ">>> Downloading goss ${goss_version} for linux/${goss_arch}..."
    curl -fsSL -o "${goss_bin}" \
        "https://github.com/goss-org/goss/releases/download/${goss_version}/goss-linux-${goss_arch}"
    chmod +x "${goss_bin}"
fi

# ---------------------------------------------------------------------------
# Check if a valid VM snapshot exists
# ---------------------------------------------------------------------------
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
    qemu-img create -f qcow2 -b "${basedisk}" -F qcow2 "${snapshot_disk}" 32G

    rm -f "${monitor_sock}"
    "${qemu}" "${qemu_args[@]}" \
        -qmp "unix:${monitor_sock},server,nowait" \
        &
    snapshot_pid=$!

    cleanup_snapshot() {
        kill "${snapshot_pid}" 2>/dev/null || true
        wait "${snapshot_pid}" 2>/dev/null || true
        rm -f "${monitor_sock}"
    }
    trap cleanup_snapshot EXIT

    sleep 2
    if ! kill -0 "${snapshot_pid}" 2>/dev/null; then
        echo >&2 ">>> QEMU failed to start"
        wait "${snapshot_pid}" || true
        exit 1
    fi

    echo ">>> Waiting for SSH..."
    if elapsed="$(wait_for_ssh 180)"; then
        echo ">>> SSH is up after ~${elapsed}s"
    else
        echo >&2 ">>> Timed out waiting for SSH"
        tail -30 "${serial_log}" >&2 || true
        exit 1
    fi

    echo ">>> Running goss validation..."
    scp "${scp_opts[@]}" "${goss_bin}" core@127.0.0.1:/tmp/goss
    scp "${scp_opts[@]}" "${root}/hack/goss.yaml" core@127.0.0.1:/tmp/goss.yaml

    rc=0
    remote "/tmp/goss --gossfile /tmp/goss.yaml validate --retry-timeout 30s --sleep 5s" \
        || rc=$?

    if [ "${rc}" -ne 0 ]; then
        echo >&2 ">>> Goss validation failed (exit code=${rc})"
        tail -50 "${serial_log}" >&2 || true
        exit "${rc}"
    fi

    echo ">>> Saving VM snapshot '${snapshot_name}'..."
    qmp "${monitor_sock}" \
        "{\"execute\":\"human-monitor-command\",\"arguments\":{\"command-line\":\"savevm ${snapshot_name}\"}}"

    echo ">>> Stopping snapshot VM..."
    qmp "${monitor_sock}" '{"execute":"quit"}'
    wait "${snapshot_pid}" 2>/dev/null || true
    rm -f "${monitor_sock}"
    trap - EXIT

    echo "${current_hash}" > "${snapshot_hash_file}"
    echo ">>> Snapshot created"
fi

# ---------------------------------------------------------------------------
# Boot VM from snapshot (instant restore, ephemeral writes)
# ---------------------------------------------------------------------------
echo ">>> Booting test VM from snapshot..."
"${qemu}" "${qemu_args[@]}" \
    -loadvm "${snapshot_name}" \
    &
qemu_pid=$!

cleanup() {
    echo ">>> Shutting down test VM (pid=${qemu_pid})..."
    kill "${qemu_pid}" 2>/dev/null || true
    wait "${qemu_pid}" 2>/dev/null || true
}
trap cleanup EXIT

sleep 2
if ! kill -0 "${qemu_pid}" 2>/dev/null; then
    echo >&2 ">>> QEMU failed to start from snapshot"
    wait "${qemu_pid}" || true
    exit 1
fi

echo ">>> Waiting for SSH (should be instant from snapshot)..."
if elapsed="$(wait_for_ssh 30)"; then
    echo ">>> SSH is up after ~${elapsed}s"
else
    echo >&2 ">>> SSH not reachable after snapshot restore"
    tail -30 "${serial_log}" >&2 || true
    exit 1
fi

# ---------------------------------------------------------------------------
# Run kerosene E2E test from host
# ---------------------------------------------------------------------------
echo ">>> Running kerosene E2E test playbook..."
cd "${root}"
"${kerosene_bin}" -i hack/test/inventory.kerosene.yml hack/test/playbook.yml

# ---------------------------------------------------------------------------
# Independent verification via SSH
# ---------------------------------------------------------------------------
echo ">>> Verifying test results..."
remote "test -f /tmp/kerosene-shell-test.txt"
remote "test \"\$(cat /tmp/kerosene-copy-content.txt)\" = 'copy-content-ok'"
remote "grep -q 'Hello from kerosene' /tmp/kerosene-copy-src.txt"
remote "grep -q 'Hello, core!' /tmp/kerosene-template.txt"
remote "test \"\$(stat -c '%U' /etc/kerosene-become-test.txt)\" = 'root'"
remote "test \"\$(cat /tmp/kerosene-task-vars.txt)\" = 'task-vars-ok'"
remote "test \"\$(cat /tmp/kerosene-task-vars-override.txt)\" = 'overridden'"
remote "test \"\$(cat /tmp/kerosene-task-vars-noleak.txt)\" = 'Fedora CoreOS'"

echo ">>> All E2E tests passed!"

# ---------------------------------------------------------------------------
# Keep VM running if requested
# ---------------------------------------------------------------------------
if [ "${KEEP_VM:-}" = "1" ]; then
    echo ">>> VM is still running (ssh -p ${ssh_port} core@127.0.0.1)"
    echo ">>> Press Ctrl-C to stop..."
    trap cleanup INT
    wait "${qemu_pid}"
fi
