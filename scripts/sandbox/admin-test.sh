#!/usr/bin/env bash
# Destructive fixtures are restricted to a disposable Linux GitHub Actions job.
# Never run against a developer's machine or an existing administrator policy.
set -euo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_OS:-}" != Linux || "${EDGEE_ADMIN_FIXTURE:-}" != 1 ]]; then
  echo 'This test requires an explicitly enabled disposable Linux GitHub Actions runner.' >&2
  exit 1
fi
if [[ -e /etc/edgee || -L /etc/edgee || -e /usr/lib/edgee-sandbox-ci || -L /usr/lib/edgee-sandbox-ci ]]; then
  echo 'Refusing to replace existing /etc/edgee or /usr/lib/edgee-sandbox-ci.' >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
built_binary="$(realpath "$repo_root/target/debug/edgee")"
edgee_binary=/usr/lib/edgee-sandbox-ci/edgee
fixture="$(mktemp -d)"
created_policy=0
created_runtime=0
cleanup() {
  if [[ "$created_policy" == 1 ]]; then sudo rm -rf -- /etc/edgee; fi
  if [[ "$created_runtime" == 1 ]]; then sudo rm -rf -- /usr/lib/edgee-sandbox-ci; fi
  rm -rf -- "$fixture"
}
trap cleanup EXIT

sudo mkdir /usr/lib/edgee-sandbox-ci
created_runtime=1
sudo mkdir /etc/edgee
created_policy=1
sudo install -m 0755 "$built_binary" "$edgee_binary"
sudo chown -hR root:root /usr/lib/edgee-sandbox-ci /etc/edgee
sudo chmod -R go-w /usr/lib/edgee-sandbox-ci /etc/edgee
# Show the trust-chain metadata in CI logs without changing shared parents.
stat -c "%A %U:%G %n" / /etc /etc/edgee /usr /usr/lib /usr/lib/edgee-sandbox-ci
cat > "$fixture/policy.toml" <<'POLICY'
mandatory = true
allowed_domains = ["api.edgee.ai"]
POLICY
sudo install -m 0644 "$fixture/policy.toml" /etc/edgee/sandbox.toml

workspace="$fixture/project with spaces"
mkdir -p "$workspace/.edgee"
git -C "$workspace" init --quiet
cd "$workspace"

expect_refusal() {
  local expected="$1"
  shift
  local output status
  set +e
  output="$(EDGEE_NO_UPDATE_CHECK=1 "$edgee_binary" "$@" 2>&1)"
  status=$?
  set -e
  if [[ "$status" == 0 || "$output" != *"$expected"* ]]; then
    printf 'Expected refusal containing %s; exit=%s; output:\n%s\n' "$expected" "$status" "$output" >&2
    exit 1
  fi
}

# None of these launches requests --sandbox. Mandatory policy must be selected
# before unsupported target handling or authentication. The alias-shaped call
# also exercises automatic Git-root discovery from a subdirectory.
mandatory_message='mandatory sandbox policy cannot be bypassed'
expect_refusal "$mandatory_message" launch --workspace "$workspace" opencode
printf 'mandatory = false\nallowed_domains = ["example.com"]\n' > .edgee/sandbox.toml
expect_refusal "$mandatory_message" launch --workspace "$workspace" opencode
export EDGEE_SANDBOX=false EDGEE_SANDBOX_DISABLED=1
expect_refusal "$mandatory_message" launch --workspace "$workspace" opencode --sandbox=false
mkdir nested
cd nested
expect_refusal "$mandatory_message" launch opencode -- --sandbox=false
cd ..
unset EDGEE_SANDBOX EDGEE_SANDBOX_DISABLED
expect_refusal 'Administrator policy requires sandboxed agent execution' relay claude
echo 'PASS mandatory policy, repository override rejection, environment override rejection, alias-shaped launch, hidden relay refusal'

sudo chmod 0666 /etc/edgee/sandbox.toml
expect_refusal 'must be root-owned and not group/world writable' launch opencode
sudo chmod 0644 /etc/edgee/sandbox.toml
sudo mv /etc/edgee/sandbox.toml /etc/edgee/real-policy.toml
sudo ln -s real-policy.toml /etc/edgee/sandbox.toml
expect_refusal 'must not be symlinks' launch opencode
sudo rm /etc/edgee/sandbox.toml
sudo mv /etc/edgee/real-policy.toml /etc/edgee/sandbox.toml
echo 'PASS writable-policy and symlink-policy rejection'

sudo chmod o+w "$edgee_binary"
expect_refusal 'administrator-installed, root-owned Edgee binary' launch opencode
sudo chmod o-w "$edgee_binary"
sudo mv "$edgee_binary" /usr/lib/edgee-sandbox-ci/edgee-native
sudo ln -s "$built_binary" "$edgee_binary"
expect_refusal 'administrator-installed, root-owned Edgee binary' launch opencode
sudo rm "$edgee_binary"
sudo mv /usr/lib/edgee-sandbox-ci/edgee-native "$edgee_binary"
expect_refusal "$mandatory_message" launch opencode
echo 'PASS writable-binary and untrusted-binary-target rejection'
echo 'PASS native administrator policy integration (Linux disposable fixture)'
