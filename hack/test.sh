#!/usr/bin/env bash
# E2E test: boot a FCOS VM, validate with goss, run kerosene playbook.
# The VM runs in snapshot mode (ephemeral) and is killed on exit.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
tmp_vm="${root}/tmp/vm"
ssh_port="${TEST_SSH_PORT:-2223}"
serial_log="${tmp_vm}/serial-test.log"
diffdisk="${tmp_vm}/diff-test.qcow2"
goss_version="v0.4.9"

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
    -drive "if=none,id=root-disk0,file=${diffdisk},format=qcow2"
    -device scsi-hd,bus=scsi0.0,drive=root-disk0

    -snapshot
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
    sha256="$(jq -ecr '.formats["qcow2.xz"].disk.sha256' <<< "${qemu_artifact}")"

    compressed="${tmp_vm}/fcos.qcow2.xz"
    curl -L -f -o "${compressed}" "${url}"

    if ! (sha256sum "${compressed}" 2>/dev/null || shasum -a 256 "${compressed}") | grep -q -F "${sha256}"; then
        echo >&2 ">>> Checksum mismatch"
        exit 1
    fi
    xz -d < "${compressed}" > "${basedisk}"
    rm -f "${compressed}"
fi

if ! [ -f "${diffdisk}" ]; then
    qemu-img create -f qcow2 -b "${basedisk}" -F qcow2 "${diffdisk}" 32G
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
# Boot VM
# ---------------------------------------------------------------------------
echo ">>> Booting test VM..."
"${qemu}" "${qemu_args[@]}" &
qemu_pid=$!

cleanup() {
    echo ">>> Shutting down test VM (pid=${qemu_pid})..."
    kill "${qemu_pid}" 2>/dev/null || true
    wait "${qemu_pid}" 2>/dev/null || true
}
trap cleanup EXIT

sleep 2
if ! kill -0 "${qemu_pid}" 2>/dev/null; then
    echo >&2 ">>> QEMU failed to start"
    wait "${qemu_pid}" || true
    exit 1
fi

# ---------------------------------------------------------------------------
# Wait for SSH
# ---------------------------------------------------------------------------
echo ">>> Waiting for SSH..."
elapsed=0
while (( elapsed < 180 )); do
    if ssh "${ssh_opts[@]}" core@127.0.0.1 true 2>/dev/null; then
        break
    fi
    sleep 5
    elapsed=$(( elapsed + 5 ))
done

if (( elapsed >= 180 )); then
    echo >&2 ">>> Timed out waiting for SSH"
    tail -30 "${serial_log}" >&2 || true
    exit 1
fi
echo ">>> SSH is up after ~${elapsed}s"

# ---------------------------------------------------------------------------
# Run goss (VM validation)
# ---------------------------------------------------------------------------
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
